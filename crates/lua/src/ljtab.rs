//! LuaJIT's table layout for the keys it hashes deterministically, numbers
//! and booleans: the array part, the hash part with its collision chains and
//! free pointer, and when and how a table is resized (`lj_tab.c` at the
//! oracle's LuaJIT commit, x86/x64 hashing). Iterating it visits keys in the
//! order `pairs` does under LuaJIT.
//!
//! Strings, tables and functions hash by a random seed or by address in
//! LuaJIT, so where they sit is not reproducible. Tables keep them apart
//! ([`crate::value::Table`]) and only tell this layout how many hash slots
//! they hold ([`Objs`]): sizes and resizes stay exact for histories without
//! deleted keys, and number keys are placed as if objects took no node.

/// A value slot: `nil` is an empty slot.
pub trait Slot: Default {
    fn is_nil(&self) -> bool;
}

impl<T> Slot for Option<T> {
    fn is_nil(&self) -> bool {
        self.is_none()
    }
}

/// A number (its bits, `-0` as `0`) or boolean key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NKey {
    Num(u64),
    Bool(bool),
}

impl NKey {
    #[inline]
    pub fn num(n: f64) -> NKey {
        NKey::Num(if n == 0.0 { 0 } else { n.to_bits() })
    }

    #[inline]
    pub fn int(i: i64) -> NKey {
        NKey::num(i as f64)
    }

    pub fn as_f64(self) -> Option<f64> {
        match self {
            NKey::Num(b) => Some(f64::from_bits(b)),
            NKey::Bool(_) => None,
        }
    }

    /// The key as an int32 index (`lj_num2int_check`), zero-extended.
    #[inline]
    fn index(self) -> Option<u32> {
        let NKey::Num(b) = self else { return None };
        let n = f64::from_bits(b);
        let i = n as i32;
        (i as f64 == n).then_some(i as u32)
    }

    /// `hashkey`: `hashnum` with the x86/x64 `hashrot`, booleans as 0 and 1.
    #[inline]
    fn hash(self) -> u32 {
        match self {
            NKey::Num(b) => {
                let (mut lo, mut hi) = (b as u32, ((b >> 32) as u32) << 1);
                lo ^= hi;
                hi = hi.rotate_left(14);
                lo = lo.wrapping_sub(hi);
                hi = hi.rotate_left(5);
                hi ^= lo;
                hi.wrapping_sub(lo.rotate_left(13))
            }
            NKey::Bool(b) => b as u32,
        }
    }
}

/// Hash slots held by keys kept outside the layout: `slots` live or dead
/// keys in the current hash part, of which `live` hold a value.
#[derive(Clone, Copy, Debug, Default)]
pub struct Objs {
    pub live: u32,
    pub slots: u32,
}

const NONE: u32 = u32::MAX;
const MAX_ABITS: usize = 28;
const MAX_ASIZE: u32 = (1 << (MAX_ABITS - 1)) + 1;

#[derive(Clone, Debug)]
struct Node<V> {
    key: Option<NKey>,
    val: V,
    next: u32,
}

/// Where a key's slot is.
#[derive(Clone, Copy)]
enum Loc {
    Arr(usize),
    Node(usize),
}

#[derive(Clone, Debug)]
pub struct LjTable<V> {
    /// Array part: slot `i` holds key `i`
    arr: Vec<V>,
    /// Hash part, or empty while no number or boolean key needed it
    nodes: Vec<Node<V>>,
    hsize: u32,
    /// Free pointer: nodes from here up all have keys
    free: u32,
    /// Nodes with a key, live or dead
    used: u32,
    /// Counts resizes; a dead object key keeps its slot within one
    pub epoch: u32,
}

impl<V> Default for LjTable<V> {
    fn default() -> Self {
        LjTable {
            arr: Vec::new(),
            nodes: Vec::new(),
            hsize: 0,
            free: 0,
            used: 0,
            epoch: 0,
        }
    }
}

/// `hsize2hbits`
pub fn hbits(n: u32) -> u32 {
    match n {
        0 => 0,
        1 => 1,
        n => 1 + fls(n - 1),
    }
}

#[inline]
fn fls(x: u32) -> u32 {
    31 - x.leading_zeros()
}

impl<V: Slot> LjTable<V> {
    /// `lj_tab_new(asize, hbits)`
    pub fn with_sizes(asize: u32, hbits: u32) -> Self {
        let mut t = LjTable::default();
        t.arr.resize_with(asize as usize, V::default);
        t.hsize = if hbits > 0 { 1 << hbits } else { 0 };
        t.free = t.hsize;
        t
    }

    pub fn asize(&self) -> usize {
        self.arr.len()
    }

    /// Number of iteration positions: the array part, then the hash part.
    pub fn positions(&self) -> usize {
        self.arr.len() + self.nodes.len()
    }

    #[inline]
    fn mp(&self, k: NKey) -> usize {
        (k.hash() & (self.hsize - 1)) as usize
    }

    #[inline]
    fn find(&self, k: NKey) -> Option<Loc> {
        if let Some(i) = k.index()
            && (i as usize) < self.arr.len()
        {
            return Some(Loc::Arr(i as usize));
        }
        if self.nodes.is_empty() {
            return None;
        }
        let mut n = self.mp(k);
        loop {
            let node = &self.nodes[n];
            if node.key == Some(k) {
                return Some(Loc::Node(n));
            }
            if node.next == NONE {
                return None;
            }
            n = node.next as usize;
        }
    }

    #[inline]
    fn at(&self, l: Loc) -> &V {
        match l {
            Loc::Arr(i) => &self.arr[i],
            Loc::Node(n) => &self.nodes[n].val,
        }
    }

    #[inline]
    fn at_mut(&mut self, l: Loc) -> &mut V {
        match l {
            Loc::Arr(i) => &mut self.arr[i],
            Loc::Node(n) => &mut self.nodes[n].val,
        }
    }

    /// The value of `k`, nil when absent.
    #[inline]
    pub fn get(&self, k: NKey) -> Option<&V> {
        self.find(k).map(|l| self.at(l)).filter(|v| !v.is_nil())
    }

    /// Whether `k` has a slot, live or dead.
    pub fn has_slot(&self, k: NKey) -> bool {
        self.find(k).is_some()
    }

    /// `lj_tab_set`: the slot of `k`, made if missing (which may resize).
    #[inline]
    pub fn slot(&mut self, k: NKey, objs: Objs) -> &mut V {
        let l = match self.find(k) {
            Some(l) => l,
            None => self.newkey(k, objs),
        };
        self.at_mut(l)
    }

    /// `lj_tab_newkey` with Brent's variation.
    fn newkey(&mut self, k: NKey, objs: Objs) -> Loc {
        if self.hsize == 0 {
            return self.rehash_and_find(Some(k), objs);
        }
        if self.nodes.is_empty() {
            self.nodes = (0..self.hsize)
                .map(|_| Node {
                    key: None,
                    val: V::default(),
                    next: NONE,
                })
                .collect();
        }
        let full = self.used + objs.slots >= self.hsize;
        let mut n = self.mp(k);
        let main = &self.nodes[n];
        if !main.val.is_nil() || (full && main.key.is_none()) {
            if full {
                return self.rehash_and_find(Some(k), objs);
            }
            loop {
                self.free -= 1;
                if self.nodes[self.free as usize].key.is_none() {
                    break;
                }
            }
            let f = self.free as usize;
            let collide = self.mp(self.nodes[n].key.expect("live node has a key"));
            if collide != n {
                // Move the colliding node to the free node, the new key takes its main node
                let mut c = collide;
                while self.nodes[c].next as usize != n {
                    c = self.nodes[c].next as usize;
                }
                self.nodes[c].next = f as u32;
                let moved = Node {
                    key: self.nodes[n].key,
                    val: std::mem::take(&mut self.nodes[n].val),
                    next: self.nodes[n].next,
                };
                self.nodes[f] = moved;
                self.nodes[n].next = NONE;
                self.used += 1;
                self.rechain(f, n);
            } else {
                self.nodes[f].next = self.nodes[n].next;
                self.nodes[n].next = f as u32;
                n = f;
            }
        }
        if self.nodes[n].key.is_none() {
            self.used += 1;
        }
        self.nodes[n].key = Some(k);
        Loc::Node(n)
    }

    /// Rechains keys behind moved node `f` whose main node is `n`, as
    /// `lj_tab_newkey` does for pseudo-resurrected keys.
    fn rechain(&mut self, mut f: usize, n: usize) {
        while self.nodes[f].next != NONE {
            let nn = self.nodes[f].next as usize;
            if !self.nodes[nn].val.is_nil() && self.mp(self.nodes[nn].key.unwrap()) == n {
                self.nodes[f].next = self.nodes[nn].next;
                self.nodes[nn].next = self.nodes[n].next;
                self.nodes[n].next = nn as u32;
                while self.nodes[f].next != NONE {
                    let nn = self.nodes[f].next as usize;
                    if !self.nodes[nn].val.is_nil() {
                        let mn = self.mp(self.nodes[nn].key.unwrap());
                        if mn != f && mn != nn {
                            self.nodes[f].next = self.nodes[nn].next;
                            self.nodes[nn].next = self.nodes[mn].next;
                            self.nodes[mn].next = nn as u32;
                            continue;
                        }
                    }
                    f = nn;
                }
                return;
            }
            f = nn;
        }
    }

    fn rehash_and_find(&mut self, k: Option<NKey>, objs: Objs) -> Loc {
        self.rehash(k, objs.live);
        let k = k.expect("rehash for a key");
        let objs = Objs {
            live: objs.live,
            slots: objs.live,
        };
        match self.find(k) {
            Some(l) => l,
            None => self.newkey(k, objs),
        }
    }

    /// Makes room for a new key kept outside the layout (`objs` before it is
    /// added); true when the table was resized, which drops dead keys.
    pub fn obj_newkey(&mut self, objs: Objs) -> bool {
        if self.hsize == 0 || self.used + objs.slots >= self.hsize {
            self.rehash(None, objs.live);
            true
        } else {
            false
        }
    }

    /// `rehashtab` for a new key `ek` (None for a key outside the layout).
    fn rehash(&mut self, ek: Option<NKey>, objs_live: u32) {
        if self.arr.is_empty() && self.nodes.is_empty() && ek.and_then(NKey::index).is_none() {
            // Nothing to count or move
            self.hsize = 1 << hbits(1 + objs_live);
            self.free = self.hsize;
            self.used = 0;
            self.epoch = self.epoch.wrapping_add(1);
            return;
        }
        let mut bins = [0u32; MAX_ABITS];
        let mut na = self.count_array(&mut bins);
        let mut total = 1 + na + objs_live;
        for n in &self.nodes {
            if !n.val.is_nil() {
                total += 1;
                na += countint(n.key, &mut bins);
            }
        }
        na += countint(ek, &mut bins);
        let (asize, in_arr) = bestasize(&bins, na);
        self.resize(asize, hbits(total - in_arr), objs_live);
    }

    fn count_array(&self, bins: &mut [u32; MAX_ABITS]) -> u32 {
        let asize = self.arr.len();
        if asize == 0 {
            return 0;
        }
        let (mut na, mut i) = (0, 0usize);
        for (b, bin) in bins.iter_mut().enumerate() {
            let mut top = 2usize << b;
            if top >= asize {
                top = asize - 1;
                if i > top {
                    break;
                }
            }
            let n = self.arr[i..=top].iter().filter(|v| !v.is_nil()).count() as u32;
            i = top + 1;
            *bin += n;
            na += n;
        }
        na
    }

    /// `lj_tab_resize`: new array and hash sizes, keys reinserted.
    pub fn resize(&mut self, asize: u32, hbits: u32, objs_live: u32) {
        let asize = asize as usize;
        if asize > self.arr.len() {
            self.arr.resize_with(asize, V::default);
        }
        let old = std::mem::take(&mut self.nodes);
        self.hsize = if hbits > 0 { 1 << hbits } else { 0 };
        self.free = self.hsize;
        self.used = 0;
        self.epoch = self.epoch.wrapping_add(1);
        let objs = Objs {
            live: objs_live,
            slots: objs_live,
        };
        if asize < self.arr.len() {
            let tail = self.arr.split_off(asize);
            for (i, v) in tail.into_iter().enumerate() {
                if !v.is_nil() {
                    *self.slot(NKey::int((asize + i) as i64), objs) = v;
                }
            }
        }
        for n in old {
            if !n.val.is_nil() {
                *self.slot(n.key.unwrap(), objs) = n.val;
            }
        }
    }

    /// `lj_tab_reasize`: an array part for keys up to `n`.
    pub fn reasize(&mut self, n: u32, objs_live: u32) {
        let hbits = if self.hsize > 0 {
            self.hsize.trailing_zeros()
        } else {
            0
        };
        self.resize(n + 1, hbits, objs_live);
    }

    /// `lj_tab_len`: a border, found as LuaJIT's interpreter finds it.
    pub fn border(&self) -> usize {
        let mut hi = self.arr.len().saturating_sub(1);
        if hi > 0 && self.arr[hi].is_nil() {
            let mut lo = 0;
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if self.arr[mid].is_nil() {
                    hi = mid
                } else {
                    lo = mid
                }
            }
            return lo;
        }
        if self.nodes.is_empty() {
            return hi;
        }
        // tab_len_slow
        let live = |i: usize| self.get(NKey::int(i as i64)).is_some();
        let mut lo = hi;
        hi += 1;
        while live(hi) {
            lo = hi;
            hi += hi;
            if hi > 0x7fff_fffd {
                let mut lo = 1;
                while live(lo) {
                    lo += 1;
                }
                return lo - 1;
            }
        }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if live(mid) { lo = mid } else { hi = mid }
        }
        lo
    }

    /// The first live entry at or after position `pos` and its position.
    #[inline]
    pub fn entry_at(&self, pos: usize) -> Option<(usize, NKey, &V)> {
        let asize = self.arr.len();
        let mut p = pos;
        while p < asize {
            if !self.arr[p].is_nil() {
                return Some((p, NKey::int(p as i64), &self.arr[p]));
            }
            p += 1;
        }
        let mut h = p - asize;
        while h < self.nodes.len() {
            let n = &self.nodes[h];
            if !n.val.is_nil() {
                return Some((asize + h, n.key.unwrap(), &n.val));
            }
            h += 1;
        }
        None
    }

    /// The live entry at position `pos`.
    #[inline]
    pub fn at_pos(&self, pos: usize) -> Option<(NKey, &V)> {
        let (k, v) = match self.arr.get(pos) {
            Some(v) => (NKey::int(pos as i64), v),
            None => {
                let n = self.nodes.get(pos - self.arr.len())?;
                (n.key?, &n.val)
            }
        };
        (!v.is_nil()).then_some((k, v))
    }

    /// `lj_tab_keyindex`: the position after `k`, None if it has no slot.
    pub fn position_after(&self, k: NKey) -> Option<usize> {
        match self.find(k)? {
            Loc::Arr(i) => Some(i + 1),
            Loc::Node(n) => Some(self.arr.len() + n + 1),
        }
    }

    /// Live entries in iteration order.
    pub fn iter(&self) -> impl Iterator<Item = (NKey, &V)> + '_ {
        let arr = self
            .arr
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_nil())
            .map(|(i, v)| (NKey::int(i as i64), v));
        let nodes = self
            .nodes
            .iter()
            .filter(|n| !n.val.is_nil())
            .map(|n| (n.key.unwrap(), &n.val));
        arr.chain(nodes)
    }

    /// Every value slot, for the cycle collector.
    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.arr.iter().chain(self.nodes.iter().map(|n| &n.val))
    }

    /// The array part's slots.
    pub fn array_part(&self) -> &[V] {
        &self.arr
    }

    /// Sets every live value to nil, keeping the layout (`wipeTable`).
    pub fn clear_values(&mut self) {
        for v in &mut self.arr {
            *v = V::default();
        }
        for n in &mut self.nodes {
            n.val = V::default();
        }
    }
}

#[inline]
fn countint(k: Option<NKey>, bins: &mut [u32; MAX_ABITS]) -> u32 {
    match k.and_then(NKey::index) {
        Some(k) if k < MAX_ASIZE => {
            bins[if k > 2 { fls(k - 1) as usize } else { 0 }] += 1;
            1
        }
        _ => 0,
    }
}

/// `bestasize`: the array size and how many keys it holds.
fn bestasize(bins: &[u32; MAX_ABITS], nn: u32) -> (u32, u32) {
    let (mut sz, mut na, mut sum) = (0, 0, 0u32);
    let mut b = 0;
    while 2 * nn > (1u32 << b) && sum != nn {
        if bins[b] > 0 {
            sum += bins[b];
            if 2 * sum > (1u32 << b) {
                sz = (2u32 << b) + 1;
                na = sum;
            }
        }
        b += 1;
    }
    (sz, na)
}

/// A map from numbers to values in LuaJIT's `pairs` order: what the typed
/// engine keeps for upstream's number-keyed tables.
#[derive(Clone, Debug)]
pub struct LjMap<V>(LjTable<Option<V>>);

impl<V> Default for LjMap<V> {
    fn default() -> Self {
        LjMap(LjTable::default())
    }
}

impl<V> LjMap<V> {
    #[inline]
    pub fn get(&self, k: u32) -> Option<&V> {
        self.0.get(NKey::int(k as i64)).and_then(Option::as_ref)
    }

    pub fn contains(&self, k: u32) -> bool {
        self.get(k).is_some()
    }

    /// `t[k] = v`
    pub fn insert(&mut self, k: u32, v: V) {
        *self.0.slot(NKey::int(k as i64), Objs::default()) = Some(v);
    }

    /// `t[k] = nil`, which in LuaJIT gives a missing key a slot too.
    pub fn remove(&mut self, k: u32) {
        *self.0.slot(NKey::int(k as i64), Objs::default()) = None;
    }

    /// Removes every entry, keeping the layout (`wipeTable`).
    pub fn clear(&mut self) {
        self.0.clear_values();
    }

    /// The entry at iteration position `i`, if set: visiting positions in
    /// order while changing entries is how upstream's `pairs` loops behave.
    #[inline]
    pub fn at(&self, i: usize) -> Option<(u32, &V)> {
        self.0
            .at_pos(i)
            .map(|(k, v)| (k.as_f64().unwrap() as u32, v.as_ref().unwrap()))
    }

    pub fn places(&self) -> usize {
        self.0.positions()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &V)> {
        self.0
            .iter()
            .map(|(k, v)| (k.as_f64().unwrap() as u32, v.as_ref().unwrap()))
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}
