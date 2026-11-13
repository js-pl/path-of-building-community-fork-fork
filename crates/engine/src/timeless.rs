//! Timeless jewels: the passives they put in place of the tree's
//! (`Data/TimelessJewelData/LegionPassives`), the nodes' look-up tables
//! (`Modules/DataLegionLookUpTableHelper.lua`, `NodeIndexMapping`) and what
//! `PassiveSpec:BuildAllDependsAndPaths` does to a conquered node.
//!
//! The look-up tables are large; the caller gives a loader of their
//! decompressed bytes by jewel name, read when a jewel first needs them
//! (and again later if the loader had none). The rest is stored in
//! `crate::codec`'s binary form ([`Timeless::encode`]).
//! The Abyss jewels (types 7 to 11) read theirs by socket, and Zorath's by
//! node (`Modules/DataAbyssJewelLookUpTableHelper.lua`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::mod_parser::ModParser;
use crate::mods::ModValue;
use crate::mods::convert::{get, number, text};
use crate::tree::Overlay;
use lua::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type Result<T> = std::result::Result<T, String>;

/// `data.timelessJewelAdditions`: the number of additions
pub const ADDITIONS: u32 = 337;

/// `data.timelessJewelTypes`
pub const TYPES: [&str; 11] = [
    "Glorious Vanity",
    "Lethal Pride",
    "Brutal Restraint",
    "Militant Faith",
    "Elegant Hubris",
    "Heroic Tragedy",
    "Abyss Tecrod",
    "Abyss Ulaman",
    "Abyss Kurgal",
    "Abyss Amanamu",
    "Abyss Zorath",
];

/// `data.timelessJewelSeedMin` and `Max`, by type from 1
const SEED_MIN: [f64; 11] = [
    100.0, 10000.0, 500.0, 2000.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0,
];
const SEED_MAX: [f64; 11] = [
    8000.0, 18000.0, 8000.0, 10000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0,
];

/// A stat of a legion passive (`stats[key]`)
#[derive(Clone, Debug, PartialEq)]
pub struct StatRange {
    pub fmt: Arc<str>,
    pub index: usize,
    pub min: f64,
    pub max: f64,
}

/// A legion passive or addition
#[derive(Clone, Debug)]
pub struct LegionNode {
    pub node: Arc<Overlay>,
    pub sorted_stats: Vec<Arc<str>>,
    /// `stats`, by key
    pub stats: Vec<(Arc<str>, StatRange)>,
}

impl LegionNode {
    fn stat(&self, key: &str) -> Option<&StatRange> {
        self.stats.iter().find(|(k, _)| &**k == key).map(|(_, s)| s)
    }
}

/// Reads a timeless jewel's decompressed look-up table by its name without
/// spaces (`LethalPride`)
pub type Loader = dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync;

/// The timeless jewels' data
pub struct Timeless {
    /// `legion.nodes` and `legion.additions`, from index 0
    pub nodes: Vec<LegionNode>,
    pub additions: Vec<LegionNode>,
    /// `nodeIDList`: each node's index and Glorious Vanity size
    node_ids: HashMap<u32, (usize, usize)>,
    size: usize,
    size_notable: usize,
    /// `nodeIDList.localIdToGlobalId`, by type from 1
    local_to_global: HashMap<usize, HashMap<u32, u32>>,
    loader: Box<Loader>,
    luts: Mutex<HashMap<usize, Arc<Vec<u8>>>>,
    abyss: Mutex<HashMap<usize, Arc<AbyssLut>>>,
}

crate::codec_struct!(StatRange {
    fmt,
    index,
    min,
    max
});
crate::codec_struct!(LegionNode {
    node,
    sorted_stats,
    stats
});

/// What [`Timeless`] holds besides the look-up tables
struct Parts {
    nodes: Vec<LegionNode>,
    additions: Vec<LegionNode>,
    node_ids: HashMap<u32, (usize, usize)>,
    size: usize,
    size_notable: usize,
    local_to_global: HashMap<usize, HashMap<u32, u32>>,
}

crate::codec_struct!(Parts {
    nodes,
    additions,
    node_ids,
    size,
    size_notable,
    local_to_global
});

impl std::fmt::Debug for Timeless {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Timeless")
    }
}

fn legion_node(v: &Value, parser: &ModParser) -> Result<LegionNode> {
    let sorted_stats = match get(v, "sortedStats") {
        Value::Table(t) => t.array().iter().map(text).collect::<Result<_>>()?,
        _ => Vec::new(),
    };
    let mut stats = Vec::new();
    if let Value::Table(t) = get(v, "stats") {
        for (k, s) in t.entries() {
            stats.push((
                text(&k.to_value())?,
                StatRange {
                    fmt: match get(&s, "fmt") {
                        Value::Nil => "d".into(),
                        f => text(&f)?,
                    },
                    index: number(&get(&s, "index")).unwrap_or(0.0) as usize,
                    min: number(&get(&s, "min")).unwrap_or(0.0),
                    max: number(&get(&s, "max")).unwrap_or(0.0),
                },
            ));
        }
    }
    Ok(LegionNode {
        node: Arc::new(Overlay::from_lua(v, parser)?),
        sorted_stats,
        stats,
    })
}

impl Timeless {
    /// The data of `LegionPassives` and `NodeIndexMapping`, with the
    /// legion passives' stats processed
    pub fn from_lua(
        legion: &Value,
        mapping: &Value,
        parser: &ModParser,
        loader: Box<Loader>,
    ) -> Result<Timeless> {
        let list = |k: &str| -> Result<Vec<LegionNode>> {
            match get(legion, k) {
                Value::Table(t) => t.array().iter().map(|v| legion_node(v, parser)).collect(),
                _ => Ok(Vec::new()),
            }
        };
        let mut node_ids = HashMap::new();
        let mut local_to_global: HashMap<usize, HashMap<u32, u32>> = HashMap::new();
        if let Value::Table(t) = mapping {
            for (k, v) in t.entries() {
                match k.to_value() {
                    Value::Number(id) => {
                        node_ids.insert(
                            id as u32,
                            (
                                number(&get(&v, "index"))? as usize,
                                number(&get(&v, "size"))? as usize,
                            ),
                        );
                    }
                    Value::Str(s) if s.as_bytes() == b"localIdToGlobalId" => {
                        for (ty, m) in v.as_table().map(|t| t.entries()).unwrap_or_default() {
                            let Value::Number(ty) = ty.to_value() else {
                                continue;
                            };
                            let map = local_to_global.entry(ty as usize).or_default();
                            for (l, g) in m.as_table().map(|t| t.entries()).unwrap_or_default() {
                                if let (Value::Number(l), Value::Number(g)) = (l.to_value(), g) {
                                    map.insert(l as u32, g as u32);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(Timeless {
            nodes: list("nodes")?,
            additions: list("additions")?,
            node_ids,
            size: number(&get(mapping, "size"))? as usize,
            size_notable: number(&get(mapping, "sizeNotable"))? as usize,
            local_to_global,
            loader,
            luts: Mutex::new(HashMap::new()),
            abyss: Mutex::new(HashMap::new()),
        })
    }

    /// Everything but the look-up tables, in the binary form
    pub fn encode(&self) -> Vec<u8> {
        crate::codec::encode(&Parts {
            nodes: self.nodes.clone(),
            additions: self.additions.clone(),
            node_ids: self.node_ids.clone(),
            size: self.size,
            size_notable: self.size_notable,
            local_to_global: self.local_to_global.clone(),
        })
    }

    /// Reads what [`Timeless::encode`] wrote, with a loader of the
    /// look-up tables
    pub fn decode(bytes: &[u8], loader: Box<Loader>) -> Result<Timeless> {
        let p: Parts = crate::codec::decode(bytes)?;
        Ok(Timeless {
            nodes: p.nodes,
            additions: p.additions,
            node_ids: p.node_ids,
            size: p.size,
            size_notable: p.size_notable,
            local_to_global: p.local_to_global,
            loader,
            luts: Mutex::new(HashMap::new()),
            abyss: Mutex::new(HashMap::new()),
        })
    }

    /// A jewel type's look-up table, loaded once it is there
    fn lut(&self, ty: usize) -> Result<Arc<Vec<u8>>> {
        let mut luts = self.luts.lock().map_err(|e| e.to_string())?;
        if let Some(lut) = luts.get(&ty) {
            return Ok(lut.clone());
        }
        let lut = (self.loader)(&TYPES[ty - 1].replace(' ', ""))
            .map(Arc::new)
            .ok_or_else(|| format!("not ported: no timeless jewel data for {}", TYPES[ty - 1]))?;
        luts.insert(ty, lut.clone());
        Ok(lut)
    }

    fn global_id(&self, ty: usize, local: u32) -> u32 {
        self.local_to_global
            .get(&ty)
            .and_then(|m| m.get(&local))
            .copied()
            .unwrap_or(local)
    }

    /// `data.readLUT(seed, nodeID, jewelType)`
    pub fn read_lut(&self, seed: f64, node_id: u32, ty: usize) -> Result<Vec<u32>> {
        let lut = self.lut(ty)?;
        let seed = if ty == 5 { seed / 20.0 } else { seed };
        let seed_offset = (seed - SEED_MIN[ty - 1]) as usize;
        let seed_size = (SEED_MAX[ty - 1] - SEED_MIN[ty - 1]) as usize + 1;
        let Some(&(index, _)) = self.node_ids.get(&node_id) else {
            return Ok(Vec::new());
        };
        let byte = |i: usize| lut.get(i).copied().map(u32::from);
        if ty == 1 {
            // Each node's lengths by seed, then each node's data by seed
            let sizes = self.size * seed_size;
            let mut start = sizes;
            let mut by_index: Vec<(usize, usize)> = self.node_ids.values().copied().collect();
            by_index.sort_unstable();
            for &(i, size) in &by_index {
                if i == index {
                    break;
                }
                start += size;
            }
            for k in 0..seed_offset {
                start += byte(index * seed_size + k).unwrap_or(0) as usize;
            }
            let len = byte(index * seed_size + seed_offset).unwrap_or(0) as usize;
            let mut out: Vec<u32> = (0..len).filter_map(|i| byte(start + i)).collect();
            let n = if len == 2 || len == 3 {
                1
            } else if len == 6 || len == 8 {
                len / 2
            } else {
                0
            };
            for v in out.iter_mut().take(n) {
                *v = self.global_id(ty, *v);
            }
            return Ok(out);
        }
        if index <= self.size_notable {
            return Ok(byte(index * seed_size + seed_offset)
                .map(|l| vec![self.global_id(ty, l)])
                .unwrap_or_default());
        }
        Ok(Vec::new())
    }
}

/// A part of an Abyss jewel's change to a node: a legion node replacing
/// it (`kind` 1) or an addition (`kind` 2), with its rolls
#[derive(Clone, Debug, PartialEq)]
pub struct AbyssComponent {
    pub kind: u8,
    pub id: u32,
    pub rolls: Vec<f64>,
}

/// An Abyss jewel's look-up table (`parseAbyssJewel`): by socket (`ABYS`,
/// types 7 to 10) or by node with the ascendancies' selections (`ABYN`,
/// Zorath)
struct AbyssLut {
    data: Arc<Vec<u8>>,
    by_node: bool,
    seed_min: f64,
    seed_max: f64,
    seed_inc: f64,
    seed_count: usize,
    blocks: HashMap<u32, usize>,
    ascendancies: Vec<(String, usize)>,
}

fn byte(d: &[u8], o: usize) -> Result<usize> {
    d.get(o)
        .map(|&b| b as usize)
        .ok_or_else(|| "Abyss jewel data ends early".to_string())
}

fn u16_at(d: &[u8], o: usize) -> Result<usize> {
    Ok(byte(d, o)? + byte(d, o + 1)? * 256)
}

/// `skipModification`
fn skip_modification(d: &[u8], mut o: usize) -> Result<usize> {
    let n = byte(d, o)?;
    o += 1;
    for _ in 0..n {
        o += 3 + byte(d, o + 2)? * 2;
    }
    Ok(o)
}

/// `skipSocketRecord`
fn skip_socket_record(d: &[u8], mut o: usize) -> Result<usize> {
    let n = byte(d, o)?;
    o += 1;
    for _ in 0..n {
        o = skip_modification(d, o + 2)?;
    }
    Ok(o)
}

/// `skipAscendancyRecord`
fn skip_ascendancy_record(d: &[u8], o: usize) -> Result<usize> {
    Ok(o + 1 + byte(d, o)? * 2)
}

impl AbyssLut {
    /// `parseAbyssJewel(jewelType, jewelData)`
    fn parse(ty: usize, data: Arc<Vec<u8>>) -> Result<AbyssLut> {
        let d = &data[..];
        let format = d.get(0..4).unwrap_or_default().to_vec();
        let version = byte(d, 4)?;
        let stored = byte(d, 5)?;
        let (seed_min, seed_max, seed_inc) = (u16_at(d, 6)?, u16_at(d, 8)?, u16_at(d, 10)?);
        if version != 1 || stored != ty || seed_inc == 0 {
            return Err(format!(
                "Invalid Abyss timeless jewel header for jewel type {ty}"
            ));
        }
        let seed_count = ((seed_max as f64 - seed_min as f64) / seed_inc as f64 + 1.0) as usize;
        let mut o = 12;
        let mut blocks = HashMap::new();
        let mut ascendancies = Vec::new();
        let by_node = match &format[..] {
            b"ABYS" => {
                let count = byte(d, o)?;
                o += 2;
                let mut ids = Vec::new();
                for _ in 0..count {
                    ids.push(u16_at(d, o)? as u32);
                    o += 2;
                }
                for id in ids {
                    blocks.insert(id, o);
                    for _ in 0..seed_count {
                        o = skip_socket_record(d, o)?;
                    }
                }
                false
            }
            b"ABYN" => {
                let count = u16_at(d, o)?;
                o += 2;
                let mut ids = Vec::new();
                for _ in 0..count {
                    ids.push(u16_at(d, o)? as u32);
                    o += 2;
                }
                for id in ids {
                    blocks.insert(id, o);
                    for _ in 0..seed_count {
                        o = skip_modification(d, o)?;
                    }
                }
                if d.get(o..o + 4) != Some(b"ASCS") {
                    return Err(format!(
                        "Missing Abyss ascendancy section for jewel type {ty}"
                    ));
                }
                o += 4;
                let count = u16_at(d, o)?;
                o += 2;
                for _ in 0..count {
                    let len = byte(d, o)?;
                    let name = d
                        .get(o + 1..o + 1 + len)
                        .ok_or("Abyss jewel data ends early")?;
                    o += len + 1;
                    ascendancies.push((String::from_utf8_lossy(name).into_owned(), o));
                    for _ in 0..seed_count {
                        o = skip_ascendancy_record(d, o)?;
                    }
                }
                true
            }
            _ => {
                return Err(format!(
                    "Unknown Abyss timeless jewel format {}",
                    String::from_utf8_lossy(&format)
                ));
            }
        };
        Ok(AbyssLut {
            data,
            by_node,
            seed_min: seed_min as f64,
            seed_max: seed_max as f64,
            seed_inc: seed_inc as f64,
            seed_count,
            blocks,
            ascendancies,
        })
    }

    /// `getSeedIndex`, from 0
    fn seed_index(&self, seed: f64) -> Option<usize> {
        let off = seed - self.seed_min;
        if off < 0.0
            || seed > self.seed_max
            || off - (off / self.seed_inc).floor() * self.seed_inc != 0.0
        {
            return None;
        }
        Some((off / self.seed_inc) as usize).filter(|&i| i < self.seed_count)
    }

    /// The offset of a seed's record in a block (`getRecordOffsets`)
    fn record(
        &self,
        block: usize,
        seed: usize,
        skip: fn(&[u8], usize) -> Result<usize>,
    ) -> Result<usize> {
        let mut o = block;
        for _ in 0..seed {
            o = skip(&self.data, o)?;
        }
        Ok(o)
    }
}

impl Timeless {
    /// An Abyss jewel type's look-up table, loaded once
    fn abyss_lut(&self, ty: usize) -> Result<Arc<AbyssLut>> {
        if let Some(l) = self.abyss.lock().map_err(|e| e.to_string())?.get(&ty) {
            return Ok(l.clone());
        }
        let lut = Arc::new(AbyssLut::parse(ty, self.lut(ty)?)?);
        self.abyss
            .lock()
            .map_err(|e| e.to_string())?
            .insert(ty, lut.clone());
        Ok(lut)
    }

    /// `readModification`
    fn read_modification(&self, d: &[u8], mut o: usize, ty: usize) -> Result<Vec<AbyssComponent>> {
        let n = byte(d, o)?;
        o += 1;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (kind, local, count) = (byte(d, o)?, byte(d, o + 1)?, byte(d, o + 2)?);
            o += 3;
            let mut rolls = Vec::with_capacity(count);
            for _ in 0..count {
                let r = u16_at(d, o)? as f64;
                o += 2;
                rolls.push(if r >= 32768.0 { r - 65536.0 } else { r });
            }
            out.push(AbyssComponent {
                kind: kind as u8,
                id: self.global_id(ty, local as u32),
                rolls,
            });
        }
        Ok(out)
    }

    /// `readNode(seed, nodeID, jewelType)`
    fn read_abyss_node(
        &self,
        lut: &AbyssLut,
        seed: f64,
        node: u32,
        ty: usize,
    ) -> Result<Vec<AbyssComponent>> {
        let (Some(s), true, Some(&block)) =
            (lut.seed_index(seed), lut.by_node, lut.blocks.get(&node))
        else {
            return Ok(Vec::new());
        };
        let o = lut.record(block, s, skip_modification)?;
        self.read_modification(&lut.data, o, ty)
    }

    /// `data.readAbyssJewelLUT(seed, socketId, jewelType, path)`: the nodes
    /// an Abyss jewel changes, in the order upstream's table takes them (a
    /// later entry for a node replaces an earlier one)
    pub fn read_abyss(
        &self,
        seed: f64,
        socket: u32,
        ty: usize,
        path: Option<&[u32]>,
    ) -> Result<Vec<(u32, Arc<Vec<AbyssComponent>>)>> {
        let mut out = Vec::new();
        if (7..=10).contains(&ty) {
            let lut = self.abyss_lut(ty)?;
            let (Some(s), false, Some(&block)) =
                (lut.seed_index(seed), lut.by_node, lut.blocks.get(&socket))
            else {
                return Ok(out);
            };
            let d = &lut.data[..];
            let mut o = lut.record(block, s, skip_socket_record)?;
            let n = byte(d, o)?;
            o += 1;
            for _ in 0..n {
                let node = u16_at(d, o)? as u32;
                o += 2;
                let m = self.read_modification(d, o, ty)?;
                o = skip_modification(d, o)?;
                out.push((node, Arc::new(m)));
            }
            return Ok(out);
        }
        let (11, Some(path)) = (ty, path) else {
            return Ok(out);
        };
        let lut = self.abyss_lut(ty)?;
        for &node in path {
            let m = self.read_abyss_node(&lut, seed, node, ty)?;
            if !m.is_empty() {
                out.push((node, Arc::new(m)));
            }
        }
        if let (Some(s), true) = (lut.seed_index(seed), lut.by_node) {
            let d = &lut.data[..];
            for (_, block) in &lut.ascendancies {
                let mut o = lut.record(*block, s, skip_ascendancy_record)?;
                let n = byte(d, o)?;
                o += 1;
                for _ in 0..n {
                    let node = u16_at(d, o)? as u32;
                    o += 2;
                    out.push((node, Arc::new(self.read_abyss_node(&lut, seed, node, ty)?)));
                }
            }
        }
        Ok(out)
    }
}

/// A timeless jewel's conquest (`jewelData.conqueredBy`)
#[derive(Clone, Debug, PartialEq)]
pub struct Conquest {
    pub seed: f64,
    /// `conqueror.type`: vaal, karui, ...
    pub kind: Arc<str>,
    /// `conqueror.id`, as Lua writes it
    pub id: Arc<str>,
    /// An Abyss jewel's change to the node (`modification`)
    pub modification: Option<Arc<Vec<AbyssComponent>>>,
}

impl Conquest {
    /// From a jewel's data
    pub fn of(jd: &crate::item::Fields) -> Option<Conquest> {
        let ModValue::Record(r) = jd.get("conqueredBy")? else {
            return None;
        };
        let seed = r.get("id")?.as_number()?;
        let ModValue::Record(c) = r.get("conqueror")? else {
            return None;
        };
        let id = match c.get("id")? {
            ModValue::Number(n) => crate::pat::num(*n).into(),
            ModValue::Str(s) => s.clone(),
            _ => return None,
        };
        let ModValue::Str(kind) = c.get("type")? else {
            return None;
        };
        Some(Conquest {
            seed,
            kind: kind.clone(),
            id,
            modification: None,
        })
    }

    /// `timelessJewelTypeByConqueror`
    pub fn jewel_type(&self) -> Option<usize> {
        Some(match &*self.kind {
            "vaal" => 1,
            "karui" => 2,
            "maraketh" => 3,
            "templar" => 4,
            "eternal" => 5,
            "kalguur" => 6,
            "abyss_murderous" => 7,
            "abyss_searching" => 8,
            "abyss_hypnotic" => 9,
            "abyss_ghastly" => 10,
            "abyss_special" => 11,
            _ => return None,
        })
    }

    /// The seed within the range of its type
    pub fn seed_in_range(&self, ty: usize) -> bool {
        let seed = if ty == 5 { self.seed / 20.0 } else { self.seed };
        seed == seed.min(SEED_MAX[ty - 1]).max(SEED_MIN[ty - 1])
    }
}

/// `replaceHelperFunc(statToFix, statKey, statMod, value)`: a stat line
/// with its range replaced by the rolled value
pub fn replace_stat(line: &str, key: &str, stat: &StatRange, value: f64) -> String {
    let mut value = value;
    if &*stat.fmt == "g" {
        if key.contains("per_minute") {
            value = crate::calc::round(value / 60.0, 1.0);
        } else if key.contains("permyriad") {
            value /= 100.0;
        } else if key.contains("_ms") {
            value /= 1000.0;
        }
    }
    let num = crate::pat::num;
    if stat.min != stat.max {
        let p = format!("%({}%-{}%)", num(stat.min), num(stat.max));
        crate::pat::gsub(line, &p, &num(value))
    } else if stat.min != value {
        crate::pat::gsub(line, &num(stat.min), &num(value))
    } else {
        line.to_string()
    }
}

impl LegionNode {
    /// The stat line `i`'s key and range (`sortedStats[i]`)
    pub fn sorted_stat(&self, i: usize) -> Option<(&str, &StatRange)> {
        let key = self.sorted_stats.get(i)?;
        Some((key, self.stat(key)?))
    }
}

impl crate::spec::PassiveSpec<'_> {
    /// The node's stat lines as parsed (`node.mods`)
    fn stat_lines(&self, i: usize) -> Vec<crate::tree::StatLine> {
        match &self.nodes[i].stats {
            Some(s) => s.mods.clone(),
            None => self.tree_node(i).mods.clone(),
        }
    }

    /// `ReplaceNode(node, legionNode)`
    fn replace_with(&mut self, i: usize, by: &LegionNode) {
        let o = &by.node;
        self.nodes[i].stats = Some(Arc::new(crate::spec::NodeStats {
            sd: o.sd.clone(),
            mods: o.mods.clone(),
            mod_list: o.mod_list.clone(),
        }));
        self.nodes[i].overlay = Some(o.clone());
    }

    /// `NodeAdditionOrReplacementFromString(node, sd, replacement)`
    fn add_from_string(&mut self, i: usize, sd: &str, replacement: bool) -> Result<()> {
        let mut lines: Vec<Arc<str>> = vec![sd.into()];
        let mut mods = Vec::new();
        let source = format!("Tree:{}", self.nodes[i].id);
        let (mod_list, _, _) =
            crate::tree::process_stats_from(self.parser, &mut lines, &mut mods, 0, Some(&source))?;
        let stats = if replacement {
            crate::spec::NodeStats {
                sd: lines,
                mods,
                mod_list,
            }
        } else {
            let mut sd = self.sd(i).to_vec();
            sd.extend(lines);
            let mut all = self.stat_lines(i);
            all.extend(mods);
            let mut list = mod_list;
            list.extend(self.mod_list(i).iter().cloned());
            crate::spec::NodeStats {
                sd,
                mods: all,
                mod_list: list,
            }
        };
        self.nodes[i].stats = Some(Arc::new(stats));
        Ok(())
    }

    /// What `BuildAllDependsAndPaths` does to a node a timeless jewel
    /// conquers
    pub(crate) fn conquer(&mut self, i: usize, c: &Conquest) -> Result<()> {
        use crate::tree::NodeType;
        let timeless = self
            .timeless
            .clone()
            .ok_or("not ported: no timeless jewel data")?;
        let ty = c.jewel_type().unwrap_or(5);
        let id = self.nodes[i].id;
        let legion = |k: u32| -> Result<&LegionNode> {
            timeless
                .nodes
                .get((k as usize).wrapping_sub(ADDITIONS as usize))
                .ok_or_else(|| format!("no legion node {k}"))
        };
        let lut = |s: &Self| -> Result<Vec<u32>> {
            if c.seed_in_range(ty) {
                let _ = s;
                timeless.read_lut(c.seed, id, ty)
            } else {
                Ok(Vec::new())
            }
        };
        if ty >= 7 {
            if self.kind(i) != NodeType::Mastery {
                for comp in c.modification.iter().flat_map(|m| m.iter()) {
                    // `data.resolveAbyssJewelComponent(component, legion)`
                    let (changed, replaces) = match comp.kind {
                        1 => (
                            timeless
                                .nodes
                                .get((comp.id as usize).wrapping_sub(ADDITIONS as usize)),
                            true,
                        ),
                        2 => (timeless.additions.get(comp.id as usize), false),
                        _ => (None, false),
                    };
                    let Some(changed) = changed else {
                        continue;
                    };
                    if replaces {
                        self.replace_with(i, changed);
                    }
                    for (k, line) in changed.node.sd.iter().enumerate() {
                        let mut line = line.to_string();
                        for (key, stat) in &changed.stats {
                            let v = comp
                                .rolls
                                .get(stat.index.wrapping_sub(1))
                                .copied()
                                .ok_or_else(|| format!("no roll {} for {key}", stat.index))?;
                            line = replace_stat(&line, key, stat, v);
                        }
                        if !replaces {
                            line.insert_str(0, " \n");
                        }
                        self.add_from_string(i, &line, replaces && k == 0)?;
                    }
                }
            }
        } else {
            match self.kind(i) {
                NodeType::Notable => {
                    let tbl = lut(self)?;
                    if tbl.is_empty() {
                    } else if ty == 1 {
                        let header = tbl.len();
                        if header == 2 || header == 3 {
                            let ln = legion(tbl[0])?;
                            self.replace_with(i, ln);
                            for (k, line) in ln.node.sd.iter().enumerate() {
                                let mut line = line.to_string();
                                if let Some((key, stat)) = ln.sorted_stat(k) {
                                    let v = tbl.get(stat.index).copied().unwrap_or(0) as f64;
                                    line = replace_stat(&line, key, stat, v);
                                }
                                self.add_from_string(i, &line, k == 0)?;
                            }
                        } else if header == 6 || header == 8 {
                            let half = header / 2;
                            let bias: i32 = tbl[..half]
                                .iter()
                                .map(|&v| if v <= 21 { 1 } else { -1 })
                                .sum();
                            let base = if bias >= 0 { 76 } else { 77 };
                            let ln = timeless.nodes.get(base).ok_or("no legion node")?;
                            self.replace_with(i, ln);
                            let mut adds: lua::ljtab::LjMap<f64> = lua::ljtab::LjMap::default();
                            for k in 0..half {
                                let roll = tbl[k + half] as f64;
                                let v = adds.get(tbl[k]).copied().map_or(roll, |x| x + roll);
                                adds.insert(tbl[k], v);
                            }
                            for (add, &val) in adds.iter() {
                                let a =
                                    timeless.additions.get(add as usize).ok_or("no addition")?;
                                for line in &a.node.sd {
                                    let mut line = line.to_string();
                                    for (key, stat) in &a.stats {
                                        line = replace_stat(&line, key, stat, val);
                                    }
                                    self.add_from_string(i, &line, false)?;
                                }
                            }
                        }
                    } else {
                        for v in tbl {
                            if v >= ADDITIONS {
                                let ln = legion(v)?;
                                self.replace_with(i, ln);
                            } else {
                                let a = timeless.additions.get(v as usize).ok_or("no addition")?;
                                for line in &a.node.sd {
                                    self.add_from_string(i, &format!(" \n{line}"), false)?;
                                }
                            }
                        }
                    }
                }
                NodeType::Keystone => {
                    let want = format!("{}_keystone_{}", c.kind, c.id);
                    if let Some(ln) = timeless.nodes.iter().find(|n| *n.node.id == *want) {
                        self.replace_with(i, ln);
                    }
                }
                NodeType::Normal => {
                    let attribute = ["Dexterity", "Intelligence", "Strength"].contains(&self.dn(i))
                        || self.is_tattoo(i);
                    match &*c.kind {
                        "vaal" => {
                            let tbl = lut(self)?;
                            if let Some(&first) = tbl.first() {
                                let ln = legion(first)?;
                                self.replace_with(i, ln);
                                let lines: Vec<Arc<str>> = self.sd(i).to_vec();
                                for (k, line) in lines.iter().enumerate() {
                                    let mut line = line.to_string();
                                    if let Some((key, stat)) = ln.sorted_stat(k) {
                                        let v = tbl.get(1).copied().unwrap_or(0) as f64;
                                        line = replace_stat(&line, key, stat, v);
                                    }
                                    self.add_from_string(i, &line, true)?;
                                }
                            }
                        }
                        "karui" | "maraketh" | "kalguur" => {
                            let line = match (&*c.kind, attribute) {
                                ("karui", true) => " \n+2 to Strength",
                                ("karui", false) => " \n+4 to Strength",
                                ("maraketh", true) => " \n+2 to Dexterity",
                                ("maraketh", false) => " \n+4 to Dexterity",
                                (_, true) => " \n1% increased Ward",
                                (_, false) => " \n2% increased Ward",
                            };
                            self.add_from_string(i, line, false)?;
                        }
                        "templar" if attribute => {
                            let ln = timeless.nodes.get(90).ok_or("no legion node")?;
                            self.replace_with(i, ln);
                        }
                        "templar" => self.add_from_string(i, " \n+5 to Devotion", false)?,
                        "eternal" => {
                            let ln = timeless.nodes.get(109).ok_or("no legion node")?;
                            self.replace_with(i, ln);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // `ReconnectNodeToClassStart(node)`
        if self.kind(i) == NodeType::Normal {
            let linked = self.tree_node(i).linked_id.clone();
            for l in linked {
                for (_, class) in &self.tree.classes {
                    if class.start_node_id == Some(l) {
                        let m = crate::mods::Mod::new(
                            &format!("Condition:ConnectedTo{}Start", class.name),
                            crate::mods::ModType::Flag,
                            true,
                            Some(&format!("Tree:{l}")),
                        );
                        let mut stats = match &self.nodes[i].stats {
                            Some(s) => (**s).clone(),
                            None => crate::spec::NodeStats {
                                sd: self.sd(i).to_vec(),
                                mods: self.stat_lines(i),
                                mod_list: self.mod_list(i).to_vec(),
                            },
                        };
                        stats.mod_list.push(m);
                        self.nodes[i].stats = Some(Arc::new(stats));
                    }
                }
            }
        }
        Ok(())
    }
}
