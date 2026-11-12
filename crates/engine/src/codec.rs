//! The compact binary form of the engine's typed game data, which the
//! importer writes at build time and the engine reads at startup.
//!
//! Integers are LEB128 varints (signed ones zigzag encoded), numbers with an
//! integral value below 2^53 are varints too, other numbers their 8 bytes.
//! Strings are interned: the first occurrence is written in full, later ones
//! as its index. Maps and sets are written in key order, so equal data gives
//! equal bytes. Compression is left to the data bundle.

use crate::mods::Sym;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

const MAGIC: &[u8; 4] = b"PBTD";
/// Bumped with every change of the format
const VERSION: u32 = 1;

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
    strs: HashMap<Arc<str>, u64>,
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    strs: Vec<Arc<str>>,
}

impl Writer {
    pub fn uint(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.buf.push(n as u8 | 0x80);
            n >>= 7;
        }
        self.buf.push(n as u8);
    }

    pub fn str(&mut self, s: &str) {
        if let Some(&i) = self.strs.get(s) {
            return self.uint(i);
        }
        let i = self.strs.len() as u64;
        self.uint(i);
        self.uint(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
        self.strs.insert(s.into(), i);
    }
}

impl Reader<'_> {
    pub fn uint(&mut self) -> Result<u64> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).ok_or("typed data is truncated")?;
            self.pos += 1;
            n |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Ok(n);
            }
        }
        Err("bad varint in typed data".into())
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let s = self
            .buf
            .get(self.pos..self.pos + n)
            .ok_or("typed data is truncated")?;
        self.pos += n;
        Ok(s)
    }

    pub fn str(&mut self) -> Result<Arc<str>> {
        let i = self.uint()? as usize;
        if let Some(s) = self.strs.get(i) {
            return Ok(s.clone());
        }
        if i != self.strs.len() {
            return Err("bad string index in typed data".into());
        }
        let n = self.uint()? as usize;
        let s: Arc<str> = std::str::from_utf8(self.bytes(n)?)
            .map_err(|_| "bad string in typed data")?
            .into();
        self.strs.push(s.clone());
        Ok(s)
    }

    pub fn count(&mut self) -> Result<usize> {
        let n = self.uint()? as usize;
        // Every element takes a byte at least
        if n > self.buf.len() - self.pos {
            return Err("bad length in typed data".into());
        }
        Ok(n)
    }
}

/// A type in the binary form
pub trait Codec: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader) -> Result<Self>;
}

/// Encodes a value with the format's header.
pub fn encode<T: Codec>(v: &T) -> Vec<u8> {
    let mut w = Writer::default();
    w.buf.extend_from_slice(MAGIC);
    w.uint(VERSION.into());
    v.put(&mut w);
    w.buf
}

/// Decodes what [`encode`] wrote.
pub fn decode<T: Codec>(bytes: &[u8]) -> Result<T> {
    let mut r = Reader {
        buf: bytes,
        pos: 0,
        strs: Vec::new(),
    };
    if r.bytes(4).ok() != Some(MAGIC) {
        return Err("not typed game data".into());
    }
    let version = r.uint()?;
    if version != u64::from(VERSION) {
        return Err(format!(
            "typed game data of format {version}, the engine reads {VERSION}"
        ));
    }
    let v = T::get(&mut r)?;
    if r.pos != bytes.len() {
        return Err("trailing bytes after typed data".into());
    }
    Ok(v)
}

/// Implements [`Codec`] for a struct field by field; a field left out
/// fails to compile.
#[macro_export]
macro_rules! codec_struct {
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl $crate::codec::Codec for $t {
            fn put(&self, w: &mut $crate::codec::Writer) {
                $( $crate::codec::Codec::put(&self.$f, w); )*
            }
            fn get(r: &mut $crate::codec::Reader) -> $crate::codec::Result<Self> {
                Ok(Self { $( $f: $crate::codec::Codec::get(r)?, )* })
            }
        }
    };
}

impl Codec for bool {
    fn put(&self, w: &mut Writer) {
        w.buf.push(*self as u8);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        match r.bytes(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("bad bool in typed data".into()),
        }
    }
}

macro_rules! codec_uint {
    ($($t:ty),*) => {$(
        impl Codec for $t {
            fn put(&self, w: &mut Writer) {
                w.uint(*self as u64);
            }
            fn get(r: &mut Reader) -> Result<Self> {
                <$t>::try_from(r.uint()?).map_err(|_| "integer out of range in typed data".into())
            }
        }
    )*};
}
codec_uint!(u8, u32, u64, usize);

impl Codec for i64 {
    fn put(&self, w: &mut Writer) {
        w.uint(zigzag(*self));
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(unzigzag(r.uint()?))
    }
}

impl Codec for f64 {
    fn put(&self, w: &mut Writer) {
        let x = *self;
        if x.fract() == 0.0 && x.abs() < 9007199254740992.0 && !(x == 0.0 && x.is_sign_negative()) {
            // Integers take the even codes, so a 1 marks raw bits
            w.uint(zigzag(x as i64) << 1);
        } else {
            w.uint(1);
            w.buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = r.uint()?;
        if n & 1 == 1 {
            let b = r.bytes(8)?;
            return Ok(f64::from_le_bytes(b.try_into().unwrap()));
        }
        Ok(unzigzag(n >> 1) as f64)
    }
}

fn zigzag(i: i64) -> u64 {
    ((i << 1) ^ (i >> 63)) as u64
}

fn unzigzag(n: u64) -> i64 {
    (n >> 1) as i64 ^ -((n & 1) as i64)
}

impl Codec for String {
    fn put(&self, w: &mut Writer) {
        w.str(self);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(r.str()?.to_string())
    }
}

impl Codec for Arc<str> {
    fn put(&self, w: &mut Writer) {
        w.str(self);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        r.str()
    }
}

impl Codec for Sym {
    fn put(&self, w: &mut Writer) {
        w.str(self.as_str());
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(Sym::new(&r.str()?))
    }
}

impl<T: Codec> Codec for Option<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            None => w.buf.push(0),
            Some(x) => {
                w.buf.push(1);
                x.put(w);
            }
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        match bool::get(r)? {
            false => Ok(None),
            true => T::get(r).map(Some),
        }
    }
}

impl<T: Codec> Codec for Box<T> {
    fn put(&self, w: &mut Writer) {
        (**self).put(w);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        T::get(r).map(Box::new)
    }
}

/// Without sharing: shared values are written once by their owner
impl<T: Codec> Codec for Arc<T> {
    fn put(&self, w: &mut Writer) {
        (**self).put(w);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        T::get(r).map(Arc::new)
    }
}

impl Codec for crate::mods::Tags {
    fn put(&self, w: &mut Writer) {
        (**self).put(w)
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Vec::get(r).map(Self::from)
    }
}

impl<T: Codec> Codec for Vec<T> {
    fn put(&self, w: &mut Writer) {
        w.uint(self.len() as u64);
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = r.count()?;
        (0..n).map(|_| T::get(r)).collect()
    }
}

impl<T: Codec, const N: usize> Codec for [T; N] {
    fn put(&self, w: &mut Writer) {
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let v = (0..N).map(|_| T::get(r)).collect::<Result<Vec<_>>>()?;
        Ok(v.try_into().unwrap_or_else(|_| unreachable!()))
    }
}

impl<A: Codec, B: Codec> Codec for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok((A::get(r)?, B::get(r)?))
    }
}

impl<K: Codec + Ord + Hash, V: Codec> Codec for HashMap<K, V> {
    fn put(&self, w: &mut Writer) {
        let mut entries: Vec<_> = self.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        w.uint(entries.len() as u64);
        for (k, v) in entries {
            k.put(w);
            v.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = r.count()?;
        let mut m = HashMap::with_capacity(n);
        for _ in 0..n {
            m.insert(K::get(r)?, V::get(r)?);
        }
        Ok(m)
    }
}

impl<K: Codec + Ord, V: Codec> Codec for BTreeMap<K, V> {
    fn put(&self, w: &mut Writer) {
        w.uint(self.len() as u64);
        for (k, v) in self {
            k.put(w);
            v.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = r.count()?;
        let mut m = BTreeMap::new();
        for _ in 0..n {
            m.insert(K::get(r)?, V::get(r)?);
        }
        Ok(m)
    }
}

impl<K: Codec + Ord + Hash> Codec for HashSet<K> {
    fn put(&self, w: &mut Writer) {
        let mut keys: Vec<_> = self.iter().collect();
        keys.sort_unstable();
        w.uint(keys.len() as u64);
        for k in keys {
            k.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = r.count()?;
        (0..n).map(|_| K::get(r)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Codec + PartialEq + std::fmt::Debug>(v: T) {
        assert_eq!(decode::<T>(&encode(&v)).unwrap(), v);
    }

    #[test]
    fn values_round_trip() {
        for x in [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            1e300,
            -9007199254740993.0,
            f64::INFINITY,
            123456789.0,
        ] {
            let back = decode::<f64>(&encode(&x)).unwrap();
            assert_eq!(back.to_bits(), x.to_bits(), "{x}");
        }
        assert!(decode::<f64>(&encode(&f64::NAN)).unwrap().is_nan());
        round_trip(vec![i64::MIN, -1, 0, 1, i64::MAX]);
        round_trip(vec!["a".to_string(), "b".into(), "a".into(), String::new()]);
        round_trip(Some((Sym::new("Life"), vec![true, false])));
        round_trip(HashMap::from([(3u32, "x".to_string()), (1, "y".into())]));
        round_trip(HashSet::from(["x".to_string(), "y".into()]));
    }

    #[test]
    fn equal_maps_equal_bytes() {
        let a: HashMap<String, f64> = (0..50).map(|i| (i.to_string(), i as f64)).collect();
        let b: HashMap<String, f64> = (0..50).rev().map(|i| (i.to_string(), i as f64)).collect();
        assert_eq!(encode(&a), encode(&b));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(decode::<f64>(b"PBTX\x01\x00").is_err());
        assert!(decode::<Vec<f64>>(&encode(&vec![1.0])[..6]).is_err());
        let mut bytes = encode(&1.0);
        bytes.push(0);
        assert!(decode::<f64>(&bytes).is_err());
    }
}
