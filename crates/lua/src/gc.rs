//! Cycle collection.
//!
//! Values are reference counted, so tables that reference each other (an
//! actor and its mod database, an environment and its actors), also through
//! closures that capture them, are never freed; upstream's calcs build such
//! an environment on every calculation. This collector finds them. Tables,
//! translated closures and the cells of captured locals are registered, and
//! a collection counts the references between them. Anything referenced more
//! often than registered objects reference it is held from outside (the Rust
//! stack, globals, native closures) and is a root; what no root reaches is
//! garbage, and clearing garbage tables and cells breaks the cycles.
//! References the collector cannot see only ever make it keep objects, so it
//! is safe at any allocation, also while Lua code runs.
//!
//! Collections are generational: most collections look only at objects
//! registered recently, which is where upstream's garbage is, and count
//! references from older objects as references from outside. Objects that
//! survive a few collections are old, and only a full collection, once the
//! old generation doubled, looks at them again.

use crate::value::{Function, Key, Table, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

enum Node {
    Table(Weak<RefCell<Table>>),
    Function(Weak<Function>),
    Cell(Weak<RefCell<Value>>),
}

enum Live {
    Table(Rc<RefCell<Table>>),
    Function(Rc<Function>),
    Cell(Rc<RefCell<Value>>),
}

impl Node {
    fn alive(&self) -> bool {
        match self {
            Node::Table(w) => w.strong_count() > 0,
            Node::Function(w) => w.strong_count() > 0,
            Node::Cell(w) => w.strong_count() > 0,
        }
    }

    fn upgrade(&self) -> Option<Live> {
        Some(match self {
            Node::Table(w) => Live::Table(w.upgrade()?),
            Node::Function(w) => Live::Function(w.upgrade()?),
            Node::Cell(w) => Live::Cell(w.upgrade()?),
        })
    }
}

impl Live {
    fn downgrade(&self) -> Node {
        match self {
            Live::Table(r) => Node::Table(Rc::downgrade(r)),
            Live::Function(r) => Node::Function(Rc::downgrade(r)),
            Live::Cell(r) => Node::Cell(Rc::downgrade(r)),
        }
    }

    fn ptr(&self) -> *const u8 {
        match self {
            Live::Table(r) => Rc::as_ptr(r) as *const u8,
            Live::Function(r) => Rc::as_ptr(r) as *const u8,
            Live::Cell(r) => Rc::as_ptr(r) as *const u8,
        }
    }

    fn strong_count(&self) -> usize {
        match self {
            Live::Table(r) => Rc::strong_count(r),
            Live::Function(r) => Rc::strong_count(r),
            Live::Cell(r) => Rc::strong_count(r),
        }
    }

    /// Calls `f` with every object this one references and how many
    /// references that edge stands for; false when it cannot be read now
    fn children(&self, f: &mut impl FnMut(*const u8, usize)) -> bool {
        match self {
            Live::Table(t) => {
                let Ok(t) = t.try_borrow() else {
                    return false;
                };
                for v in t.array() {
                    value(v, f);
                }
                for (k, v) in t.raw_entries() {
                    match k {
                        Key::Table(k) => f(Rc::as_ptr(&k.0.0) as *const u8, 1),
                        Key::Function(k) => f(Rc::as_ptr(&k.0) as *const u8, 1),
                        _ => {}
                    }
                    value(v, f);
                }
                if let Some(m) = &t.meta {
                    f(Rc::as_ptr(&m.0) as *const u8, 1);
                }
                true
            }
            // Each cell is held by the closure's upvalue struct too
            Live::Function(func) => {
                for c in &func.upvalues {
                    f(Rc::as_ptr(c) as *const u8, 2);
                }
                true
            }
            Live::Cell(c) => {
                let Ok(v) = c.try_borrow() else {
                    return false;
                };
                value(&v, f);
                true
            }
        }
    }

    fn clear(&self) -> Option<Garbage> {
        match self {
            Live::Table(t) => t
                .try_borrow_mut()
                .ok()
                .map(|mut b| Garbage::Table(std::mem::take(&mut *b))),
            Live::Cell(c) => c
                .try_borrow_mut()
                .ok()
                .map(|mut b| Garbage::Value(std::mem::take(&mut *b))),
            Live::Function(_) => None,
        }
    }
}

/// Contents taken out of garbage, dropped after the collection
enum Garbage {
    Table(#[allow(dead_code)] Table),
    Value(#[allow(dead_code)] Value),
}

fn value(v: &Value, f: &mut impl FnMut(*const u8, usize)) {
    match v {
        Value::Table(t) => f(Rc::as_ptr(&t.0) as *const u8, 1),
        Value::Function(func) => f(Rc::as_ptr(func) as *const u8, 1),
        _ => {}
    }
}

struct Registry {
    /// Objects of the young generation, with the minor collections each
    /// survived
    young: Vec<(Node, u8)>,
    /// Objects that survived [`PROMOTE`] collections
    old: Vec<Node>,
    /// Objects registered since the last collection
    since: usize,
    /// Registrations that trigger the next collection
    threshold: usize,
    /// Size of the old generation that makes the next collection a full one
    old_limit: usize,
    collecting: bool,
}

const MIN_THRESHOLD: usize = 1 << 21;

/// Minor collections an object survives before it is old
const PROMOTE: u8 = 2;

thread_local! {
    static REGISTRY: RefCell<Registry> = const {
        RefCell::new(Registry {
            young: Vec::new(),
            old: Vec::new(),
            since: 0,
            threshold: MIN_THRESHOLD,
            old_limit: MIN_THRESHOLD,
            collecting: false,
        })
    };
    static TOTAL: RefCell<(usize, usize)> = const { RefCell::new((0, 0)) };
}

fn register(node: Node) {
    let due = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.young.push((node, 0));
        r.since += 1;
        r.since >= r.threshold && !r.collecting
    });
    if due {
        let full = REGISTRY.with(|r| {
            let r = r.borrow();
            r.old.len() >= r.old_limit
        });
        run(full);
    }
}

pub(crate) fn register_table(t: &Rc<RefCell<Table>>) {
    register(Node::Table(Rc::downgrade(t)));
}

pub(crate) fn register_function(f: &Rc<Function>) {
    register(Node::Function(Rc::downgrade(f)));
}

pub(crate) fn register_cell(c: &Rc<RefCell<Value>>) {
    register(Node::Cell(Rc::downgrade(c)));
}

/// What a collection did.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub live: usize,
    pub cleared: usize,
}

/// Collections run and objects cleared so far.
pub fn totals() -> (usize, usize) {
    TOTAL.with(|t| *t.borrow())
}

/// Clears what only garbage references, in both generations.
pub fn collect() -> Stats {
    run(true)
}

/// A collection. A minor one looks at the young generation only and takes
/// every reference from old objects for one from outside, so it keeps
/// whatever old objects reference; a full one looks at both.
fn run(full: bool) -> Stats {
    // The objects looked at: young ones first, with their ages
    let (live, ages): (Vec<Live>, Vec<u8>) = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.collecting = true;
        let young = std::mem::take(&mut r.young);
        let old = std::mem::take(&mut r.old);
        let mut live = Vec::with_capacity(young.len() + if full { old.len() } else { 0 });
        let mut ages = Vec::with_capacity(live.capacity());
        for (node, age) in &young {
            if let Some(l) = node.upgrade() {
                live.push(l);
                ages.push(*age);
            }
        }
        let mut kept_old = Vec::new();
        if full {
            for node in &old {
                if let Some(l) = node.upgrade() {
                    live.push(l);
                    ages.push(PROMOTE);
                }
            }
        } else {
            kept_old = old;
            kept_old.retain(Node::alive);
        }
        r.old = kept_old;
        (live, ages)
    });
    let n = live.len();
    let mut index: HashMap<
        *const u8,
        usize,
        std::hash::BuildHasherDefault<crate::value::FxHasher>,
    > = HashMap::with_capacity_and_hasher(n, Default::default());
    for (i, o) in live.iter().enumerate() {
        index.insert(o.ptr(), i);
    }
    // References from the other objects looked at
    let mut internal = vec![0usize; n];
    let mut readable = vec![false; n];
    for (i, o) in live.iter().enumerate() {
        readable[i] = o.children(&mut |p, count| {
            if let Some(&j) = index.get(&p) {
                internal[j] += count;
            }
        });
    }
    // Roots: held from outside (the extra count is `live` itself), or not
    // readable; then everything they reach
    let mut marked = vec![false; n];
    let mut stack = Vec::new();
    for i in 0..n {
        if !readable[i] || live[i].strong_count() - 1 > internal[i] {
            marked[i] = true;
            stack.push(i);
        }
    }
    while let Some(i) = stack.pop() {
        live[i].children(&mut |p, _| {
            if let Some(&j) = index.get(&p)
                && !marked[j]
            {
                marked[j] = true;
                stack.push(j);
            }
        });
    }
    // Clear the garbage; its contents drop once no borrow is held
    let mut garbage = Vec::new();
    for i in 0..n {
        if !marked[i]
            && let Some(g) = live[i].clear()
        {
            garbage.push(g);
        }
    }
    let stats = Stats {
        live: marked.iter().filter(|&&m| m).count(),
        cleared: garbage.len(),
    };
    // Survivors age; old enough, they join the old generation
    let mut young = Vec::new();
    let mut promoted = Vec::new();
    for i in 0..n {
        if marked[i] {
            let node = live[i].downgrade();
            if ages[i] + 1 >= PROMOTE {
                promoted.push(node);
            } else {
                young.push((node, ages[i] + 1));
            }
        }
    }
    drop(garbage);
    drop(live);
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        // Objects registered while collecting stay young
        young.append(&mut r.young);
        r.young = young;
        r.old.append(&mut promoted);
        r.since = 0;
        r.threshold = r.young.len().max(MIN_THRESHOLD);
        if full {
            r.old_limit = (2 * r.old.len()).max(MIN_THRESHOLD);
        }
        r.collecting = false;
    });
    TOTAL.with(|t| {
        let mut t = t.borrow_mut();
        t.0 += 1;
        t.1 += stats.cleared;
    });
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::{cell, func_up};
    use crate::value::TableRef;

    #[test]
    fn cycles_are_cleared_and_held_objects_kept() {
        // A garbage cycle
        let weak = {
            let a = TableRef::new();
            let b = TableRef::new();
            a.set_str("b", Value::Table(b.clone()));
            b.set_str("a", Value::Table(a.clone()));
            Rc::downgrade(&a.0)
        };
        // A garbage cycle through a closure: the table holds a closure that
        // captures the table
        let weak_closure = {
            let t = TableRef::new();
            let c = cell(Value::Table(t.clone()));
            let held = c.clone();
            let f = func_up("f", vec![c], move |_| Ok(vec![held.borrow().clone()]));
            t.set_str("f", f);
            Rc::downgrade(&t.0)
        };
        // A cycle held from the stack, with a child only it references
        let held = TableRef::new();
        let other = TableRef::new();
        held.set_str("other", Value::Table(other.clone()));
        other.set_str("held", Value::Table(held.clone()));
        let child = TableRef::new();
        child.set_str("x", Value::Number(1.0));
        other.set_str("child", Value::Table(child));
        drop(other);
        // A table used as a key and as a metatable
        let key = TableRef::new();
        held.set(Value::Table(key.clone()), Value::Bool(true));
        drop(key);
        let meta = TableRef::new();
        held.borrow_mut().meta = Some(meta);
        // A closure held from the stack keeps what it captures
        let captured = TableRef::new();
        captured.set_str("y", Value::Number(2.0));
        let c = cell(Value::Table(captured.clone()));
        drop(captured);
        let inner = c.clone();
        let closure = func_up("g", vec![c], move |_| Ok(vec![inner.borrow().clone()]));

        assert!(weak.upgrade().is_some());
        assert!(weak_closure.upgrade().is_some());
        collect();
        assert!(weak.upgrade().is_none(), "the garbage cycle was not freed");
        assert!(
            weak_closure.upgrade().is_none(),
            "the cycle through a closure was not freed"
        );
        let Value::Table(other) = held.get_str("other") else {
            panic!("held table was cleared")
        };
        let Value::Table(child) = other.get_str("child") else {
            panic!("child of a held table was cleared")
        };
        assert!(matches!(child.get_str("x"), Value::Number(n) if n == 1.0));
        assert_eq!(held.entries().len(), 2);
        assert!(held.borrow().meta.is_some());
        let r = crate::rt::call(&closure, &[]).unwrap();
        let Value::Table(t) = &r[0] else {
            panic!("captured table was cleared")
        };
        assert!(matches!(t.get_str("y"), Value::Number(n) if n == 2.0));
    }

    #[test]
    fn minor_collections_keep_what_old_objects_reference() {
        // An old cycle, held from the stack
        let old = TableRef::new();
        let other = TableRef::new();
        old.set_str("other", Value::Table(other.clone()));
        other.set_str("old", Value::Table(old.clone()));
        drop(other);
        for _ in 0..PROMOTE {
            run(false);
        }
        REGISTRY.with(|r| assert!(r.borrow().young.is_empty()));
        // A young object only the old cycle references, and a young garbage cycle
        let child = TableRef::new();
        child.set_str("x", Value::Number(1.0));
        old.set_str("child", Value::Table(child.clone()));
        let weak_child = Rc::downgrade(&child.0);
        drop(child);
        let weak = {
            let a = TableRef::new();
            let b = TableRef::new();
            a.set_str("b", Value::Table(b.clone()));
            b.set_str("a", Value::Table(a.clone()));
            Rc::downgrade(&a.0)
        };
        run(false);
        assert!(
            weak.upgrade().is_none(),
            "the young garbage cycle was not freed"
        );
        assert!(
            weak_child.upgrade().is_some(),
            "a child of an old table was freed"
        );
        // Old garbage waits for a full collection
        let weak_old = Rc::downgrade(&old.0);
        drop(old);
        run(false);
        assert!(weak_old.upgrade().is_some());
        collect();
        assert!(
            weak_old.upgrade().is_none(),
            "the old garbage cycle was not freed"
        );
        assert!(weak_child.upgrade().is_none());
    }
}
