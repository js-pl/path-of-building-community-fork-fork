//! The game data items need, as typed data: item bases, affixes, the
//! uniques that craft like rares, Foulborn mod pairs, cluster jewels, mod
//! scalability. Read from upstream's processed `data` table (what
//! `Modules/Data.lua` builds).
//!
//! Upstream visits some of these tables with `pairs`, and results can depend
//! on the order (the best partial base name match, which affix a mod name
//! picks); they are kept in the order the data lists them.

use crate::codec::{self, Codec, Reader, Writer};
use crate::mods::convert::{array, get, number, opt_number, text};
use lua::{Key, Value};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Req {
    pub level: Option<f64>,
    pub str: Option<f64>,
    pub dex: Option<f64>,
    pub int: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlaskBase {
    pub life: Option<f64>,
    pub mana: Option<f64>,
    pub duration: f64,
    pub charges_used: f64,
    pub charges_max: f64,
    pub buff: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TinctureBase {
    pub mana_burn: f64,
    pub cooldown: f64,
    pub buff: Option<Vec<String>>,
}

/// An item base (`data.itemBases[name]`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Base {
    pub name: String,
    pub typ: String,
    pub sub_type: Option<String>,
    pub tags: HashSet<String>,
    /// Influence key (`shaper`, ...) to its spawn tag
    pub influence_tags: Option<HashMap<String, String>>,
    pub req: Req,
    pub socket_limit: Option<f64>,
    /// `PhysicalMin`, `AttackRateBase`, ...
    pub weapon: Option<HashMap<String, f64>>,
    /// `ArmourBaseMin`, `BlockChance`, `MovementPenalty`, ...
    pub armour: Option<HashMap<String, f64>>,
    pub flask: Option<FlaskBase>,
    pub tincture: Option<TinctureBase>,
    pub enchant: Option<String>,
    pub enchant_mod_types: Option<Vec<Vec<String>>>,
}

/// An affix or other item mod (`data.itemMods.<group>[id]`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Affix {
    pub id: String,
    /// `"Prefix"`, `"Suffix"`, ...
    pub typ: Option<String>,
    /// The affix name, e.g. `of the Brute`
    pub affix: Option<String>,
    pub lines: Vec<String>,
    pub stat_order: Vec<f64>,
    pub mod_tags: Vec<String>,
    pub weight_key: Vec<String>,
    pub weight_val: Vec<f64>,
    pub weight_multiplier_key: Vec<String>,
    pub weight_multiplier_val: Vec<f64>,
}

/// A table of affixes, in data order
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AffixSet {
    /// Sets share the affixes of the same table (`itemMods.Item` holds
    /// the other groups')
    pub list: Vec<Rc<Affix>>,
    index: HashMap<String, usize>,
}

impl AffixSet {
    pub fn get(&self, id: &str) -> Option<&Affix> {
        self.index.get(id).map(|&i| &*self.list[i])
    }

    /// `seen`: the affixes read so far, by table
    fn from_lua(v: &Value, seen: &mut HashMap<usize, Rc<Affix>>) -> Result<AffixSet> {
        let mut set = AffixSet::default();
        for (k, m) in pairs(v) {
            let Key::Str(id) = k else {
                return Err(format!("affix key {k:?}"));
            };
            let id = id.to_str_lossy().into_owned();
            let addr = m.as_table().map(|t| Rc::as_ptr(&t.0) as usize);
            if let Some(a) = addr.and_then(|p| seen.get(&p)).filter(|a| a.id == id) {
                set.index.insert(id, set.list.len());
                set.list.push(a.clone());
                continue;
            }
            let strings = |field: &str| -> Result<Vec<String>> {
                array(&get(&m, field)).iter().map(string).collect()
            };
            let numbers = |field: &str| -> Result<Vec<f64>> {
                array(&get(&m, field)).iter().map(number).collect()
            };
            let affix = Affix {
                id: id.clone(),
                typ: opt_string(&get(&m, "type"))?,
                affix: opt_string(&get(&m, "affix"))?,
                lines: array(&m).iter().map(string).collect::<Result<_>>()?,
                stat_order: numbers("statOrder")?,
                mod_tags: strings("modTags")?,
                weight_key: strings("weightKey")?,
                weight_val: numbers("weightVal")?,
                weight_multiplier_key: strings("weightMultiplierKey")?,
                weight_multiplier_val: numbers("weightMultiplierVal")?,
            };
            let affix = Rc::new(affix);
            if let Some(p) = addr {
                seen.insert(p, affix.clone());
            }
            set.index.insert(id, set.list.len());
            set.list.push(affix);
        }
        Ok(set)
    }
}

/// `data.rareLikeUniques[title]`: a unique crafted with the rare controls
#[derive(Clone, Debug, PartialEq)]
pub struct RareLikeUnique {
    pub affixes: Rc<AffixSet>,
    /// The spawn tags of the bases its mods come from, if not its own
    pub valid_base_tags: Option<Vec<HashSet<String>>>,
    pub prefix_limit: f64,
    pub suffix_limit: f64,
    pub ignore_mod_type: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClusterJewel {
    pub size: String,
    pub size_index: f64,
    pub min_nodes: f64,
    pub max_nodes: f64,
    pub skills: HashSet<String>,
    /// The skills of the small nodes, by id
    pub skill_data: HashMap<String, ClusterSkill>,
    pub small_indicies: Vec<i64>,
    pub notable_indicies: Vec<i64>,
    pub socket_indicies: Vec<i64>,
    pub total_indicies: i64,
}

/// A cluster jewel's small node skill
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterSkill {
    pub name: String,
    pub stats: Vec<String>,
    /// Whether it has a mastery (`masteryIcon`)
    pub mastery: bool,
}

/// One value of a line in `data.modScalability`
#[derive(Clone, Debug, PartialEq)]
pub struct Scalability {
    pub is_scalable: bool,
    pub formats: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemData {
    bases: Vec<Rc<Base>>,
    base_index: HashMap<String, usize>,
    /// `data.itemMods`, by group (`Item`, `Explicit`, `Flask`, ...)
    pub item_mods: HashMap<String, Rc<AffixSet>>,
    pub rare_like_uniques: HashMap<String, RareLikeUnique>,
    /// Unique title to its (original mod, Foulborn mod) pairs
    pub foulborn_map: HashMap<String, Vec<(String, String)>>,
    pub cluster_jewels: HashMap<String, ClusterJewel>,
    /// `data.clusterJewels.notableSortOrder`
    pub cluster_notable_sort_order: HashMap<String, f64>,
    /// `data.clusterJewels.orbitOffsets`: by proxy node, the offset of each
    /// size index
    pub cluster_orbit_offsets: HashMap<u32, HashMap<i64, i64>>,
    pub mod_scalability: HashMap<String, Vec<Scalability>>,
    pub high_precision_mods: HashMap<(String, String), f64>,
    /// `data.itemTagSpecial` and `data.itemTagSpecialExclusionPattern`:
    /// by tag and lower case slot, the patterns of lines that do and do not
    /// count as having the tag
    pub item_tag_special: HashMap<(String, String), Vec<String>>,
    pub item_tag_special_exclusion: HashMap<(String, String), Vec<String>>,
    pub default_high_precision: f64,
    /// The keys of `colorCodes`, which name valid rarities
    pub color_codes: HashSet<String>,
    pub caster_tag_crucible_uniques: HashSet<String>,
    pub minion_tag_crucible_uniques: HashSet<String>,
    /// The lowest stat order of each unique mod line, exactly and with its
    /// numbers replaced (`uniqueModStatOrder` of `Classes/Item.lua`)
    pub unique_stat_order_exact: HashMap<String, f64>,
    pub unique_stat_order_normalised: HashMap<String, f64>,
}

fn pairs(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn string(v: &Value) -> Result<String> {
    text(v).map(|s| s.to_string())
}

fn opt_string(v: &Value) -> Result<Option<String>> {
    if v.is_nil() {
        Ok(None)
    } else {
        string(v).map(Some)
    }
}

fn key_string(k: &Key) -> Result<String> {
    match k {
        Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
        k => Err(format!("expected a string key, got {k:?}")),
    }
}

fn truthy_set(v: &Value) -> Result<HashSet<String>> {
    pairs(v)
        .into_iter()
        .filter(|(_, x)| x.truthy())
        .map(|(k, _)| key_string(&k))
        .collect()
}

fn number_map(v: &Value) -> Result<HashMap<String, f64>> {
    pairs(v)
        .into_iter()
        .map(|(k, x)| Ok((key_string(&k)?, number(&x)?)))
        .collect()
}

fn opt_lines(v: &Value) -> Result<Option<Vec<String>>> {
    if v.is_nil() {
        Ok(None)
    } else {
        array(v).iter().map(string).collect::<Result<_>>().map(Some)
    }
}

/// `normaliseModLine` of `Classes/Item.lua`: numbers and ranges as `#`,
/// lower case, on one line
pub fn normalise_mod_line(line: &str) -> String {
    use crate::pat::gsub;
    gsub(
        &gsub(&gsub(line, "%d+%.?%d*", "#"), "%(%-?#%-#%)", "#").to_ascii_lowercase(),
        "\n",
        " ",
    )
}

impl Base {
    fn from_lua(name: &str, v: &Value) -> Result<Base> {
        let req = get(v, "req");
        let flask = get(v, "flask");
        let tincture = get(v, "tincture");
        let opt_map = |field: &str| -> Result<Option<HashMap<String, f64>>> {
            let x = get(v, field);
            if x.is_nil() {
                Ok(None)
            } else {
                number_map(&x).map(Some)
            }
        };
        let influence = get(v, "influenceTags");
        Ok(Base {
            name: name.to_string(),
            typ: string(&get(v, "type"))?,
            sub_type: opt_string(&get(v, "subType"))?,
            tags: truthy_set(&get(v, "tags"))?,
            influence_tags: if influence.is_nil() {
                None
            } else {
                Some(
                    pairs(&influence)
                        .into_iter()
                        .map(|(k, x)| Ok((key_string(&k)?, string(&x)?)))
                        .collect::<Result<_>>()?,
                )
            },
            req: Req {
                level: opt_number(&get(&req, "level"))?,
                str: opt_number(&get(&req, "str"))?,
                dex: opt_number(&get(&req, "dex"))?,
                int: opt_number(&get(&req, "int"))?,
            },
            socket_limit: opt_number(&get(v, "socketLimit"))?,
            weapon: opt_map("weapon")?,
            armour: opt_map("armour")?,
            flask: if flask.is_nil() {
                None
            } else {
                Some(FlaskBase {
                    life: opt_number(&get(&flask, "life"))?,
                    mana: opt_number(&get(&flask, "mana"))?,
                    duration: number(&get(&flask, "duration"))?,
                    charges_used: number(&get(&flask, "chargesUsed"))?,
                    charges_max: number(&get(&flask, "chargesMax"))?,
                    buff: opt_lines(&get(&flask, "buff"))?,
                })
            },
            tincture: if tincture.is_nil() {
                None
            } else {
                Some(TinctureBase {
                    mana_burn: number(&get(&tincture, "manaBurn"))?,
                    cooldown: number(&get(&tincture, "cooldown"))?,
                    buff: opt_lines(&get(&tincture, "buff"))?,
                })
            },
            enchant: opt_string(&get(v, "enchant"))?,
            enchant_mod_types: {
                let x = get(v, "enchantModTypes");
                if x.is_nil() {
                    None
                } else {
                    Some(
                        array(&x)
                            .iter()
                            .map(|t| array(t).iter().map(string).collect())
                            .collect::<Result<_>>()?,
                    )
                }
            },
        })
    }
}

impl ItemData {
    pub fn base(&self, name: &str) -> Option<&Rc<Base>> {
        self.base_index.get(name).map(|&i| &self.bases[i])
    }

    /// The bases, in data order
    pub fn bases(&self) -> &[Rc<Base>] {
        &self.bases
    }

    /// Reads the item data from upstream's processed `data` table and its
    /// `colorCodes`.
    pub fn from_lua(data: &Value, color_codes: &Value) -> Result<ItemData> {
        let mut d = ItemData {
            color_codes: pairs(color_codes)
                .iter()
                .map(|(k, _)| key_string(k))
                .collect::<Result<_>>()?,
            ..ItemData::default()
        };
        for (k, v) in pairs(&get(data, "itemBases")) {
            let name = key_string(&k)?;
            let base = Base::from_lua(&name, &v).map_err(|e| format!("base {name}: {e}"))?;
            d.base_index.insert(name, d.bases.len());
            d.bases.push(Rc::new(base));
        }
        // Tables shared between groups (and with rareLikeUniques) are read once
        let mut by_table: Vec<(Value, Rc<AffixSet>)> = Vec::new();
        let mut affixes = HashMap::new();
        let mut set_of = |v: &Value| -> Result<Rc<AffixSet>> {
            if let Some((_, s)) = by_table.iter().find(|(t, _)| t.raw_eq(v)) {
                return Ok(s.clone());
            }
            let s = Rc::new(AffixSet::from_lua(v, &mut affixes)?);
            by_table.push((v.clone(), s.clone()));
            Ok(s)
        };
        for (k, v) in pairs(&get(data, "itemMods")) {
            let group = key_string(&k)?;
            let set = set_of(&v).map_err(|e| format!("itemMods.{group}: {e}"))?;
            d.item_mods.insert(group, set);
        }
        for (k, v) in pairs(&get(data, "rareLikeUniques")) {
            let title = key_string(&k)?;
            let valid = get(&v, "validBases");
            d.rare_like_uniques.insert(
                title,
                RareLikeUnique {
                    affixes: set_of(&get(&v, "affixes"))?,
                    valid_base_tags: if valid.is_nil() {
                        None
                    } else {
                        Some(
                            array(&valid)
                                .iter()
                                .map(|b| truthy_set(&get(&get(b, "base"), "tags")))
                                .collect::<Result<_>>()?,
                        )
                    },
                    prefix_limit: number(&get(&v, "prefixLimit"))?,
                    suffix_limit: number(&get(&v, "suffixLimit"))?,
                    ignore_mod_type: get(&v, "ignoreModType").truthy(),
                },
            );
        }
        for (k, v) in pairs(&get(data, "foulbornMap")) {
            let title = key_string(&k)?;
            let list = pairs(&v)
                .into_iter()
                .map(|(o, f)| Ok((key_string(&o)?, string(&f)?)))
                .collect::<Result<_>>()?;
            d.foulborn_map.insert(title, list);
        }
        for (k, v) in pairs(&get(&get(data, "clusterJewels"), "jewels")) {
            d.cluster_jewels.insert(
                key_string(&k)?,
                ClusterJewel {
                    size: string(&get(&v, "size"))?,
                    size_index: number(&get(&v, "sizeIndex"))?,
                    min_nodes: number(&get(&v, "minNodes"))?,
                    max_nodes: number(&get(&v, "maxNodes"))?,
                    skills: pairs(&get(&v, "skills"))
                        .iter()
                        .map(|(k, _)| key_string(k))
                        .collect::<Result<_>>()?,
                    skill_data: pairs(&get(&v, "skills"))
                        .iter()
                        .map(|(k, sk)| {
                            Ok((
                                key_string(k)?,
                                ClusterSkill {
                                    name: string(&get(sk, "name"))?,
                                    stats: array(&get(sk, "stats"))
                                        .iter()
                                        .map(string)
                                        .collect::<Result<_>>()?,
                                    mastery: !get(sk, "masteryIcon").is_nil(),
                                },
                            ))
                        })
                        .collect::<Result<_>>()?,
                    small_indicies: ints(&get(&v, "smallIndicies"))?,
                    notable_indicies: ints(&get(&v, "notableIndicies"))?,
                    socket_indicies: ints(&get(&v, "socketIndicies"))?,
                    total_indicies: number(&get(&v, "totalIndicies"))? as i64,
                },
            );
        }
        let cj = get(data, "clusterJewels");
        for (k, v) in pairs(&get(&cj, "notableSortOrder")) {
            d.cluster_notable_sort_order
                .insert(key_string(&k)?, number(&v)?);
        }
        for (k, v) in pairs(&get(&cj, "orbitOffsets")) {
            let Key::Int(proxy) = k else { continue };
            let mut offsets = HashMap::new();
            for (s, o) in pairs(&v) {
                if let Key::Int(s) = s {
                    offsets.insert(s, number(&o)? as i64);
                }
            }
            d.cluster_orbit_offsets.insert(proxy as u32, offsets);
        }
        for (k, v) in pairs(&get(data, "modScalability")) {
            let list = array(&v)
                .iter()
                .map(|x| {
                    Ok(Scalability {
                        is_scalable: get(x, "isScalable").truthy(),
                        formats: array(&get(x, "formats"))
                            .iter()
                            .map(string)
                            .collect::<Result<_>>()?,
                    })
                })
                .collect::<Result<_>>()?;
            d.mod_scalability.insert(key_string(&k)?, list);
        }
        for (name, types) in pairs(&get(data, "highPrecisionMods")) {
            for (typ, p) in pairs(&types) {
                d.high_precision_mods
                    .insert((key_string(&name)?, key_string(&typ)?), number(&p)?);
            }
        }
        d.default_high_precision = number(&get(data, "defaultHighPrecision"))?;
        for (field, out) in [
            ("itemTagSpecial", &mut d.item_tag_special),
            (
                "itemTagSpecialExclusionPattern",
                &mut d.item_tag_special_exclusion,
            ),
        ] {
            for (tag, slots) in pairs(&get(data, field)) {
                for (slot, patterns) in pairs(&slots) {
                    out.insert(
                        (key_string(&tag)?, key_string(&slot)?),
                        array(&patterns).iter().map(string).collect::<Result<_>>()?,
                    );
                }
            }
        }
        d.caster_tag_crucible_uniques = truthy_set(&get(data, "casterTagCrucibleUniques"))?;
        d.minion_tag_crucible_uniques = truthy_set(&get(data, "minionTagCrucibleUniques"))?;
        if let Some(exclusive) = d.item_mods.get("ItemExclusive").cloned() {
            for m in &exclusive.list {
                for (i, line) in m.lines.iter().enumerate() {
                    let Some(&order) = m.stat_order.get(i) else {
                        continue;
                    };
                    let exact = line.to_ascii_lowercase().replace('\n', " ");
                    let e = d
                        .unique_stat_order_exact
                        .entry(exact)
                        .or_insert(f64::INFINITY);
                    *e = e.min(order);
                    let n = d
                        .unique_stat_order_normalised
                        .entry(normalise_mod_line(line))
                        .or_insert(f64::INFINITY);
                    *n = n.min(order);
                }
            }
        }
        Ok(d)
    }
}

impl crate::mods::list::Precision for ItemData {
    fn high_precision(&self, name: crate::mods::Sym, typ: crate::mods::ModType) -> Option<f64> {
        self.high_precision_mods
            .get(&(name.as_str().to_string(), typ.name().to_string()))
            .copied()
    }

    fn default_high_precision(&self) -> f64 {
        self.default_high_precision
    }
}

crate::codec_struct!(Req {
    level,
    str,
    dex,
    int
});
crate::codec_struct!(FlaskBase {
    life,
    mana,
    duration,
    charges_used,
    charges_max,
    buff
});
crate::codec_struct!(TinctureBase {
    mana_burn,
    cooldown,
    buff
});
crate::codec_struct!(Base {
    name,
    typ,
    sub_type,
    tags,
    influence_tags,
    req,
    socket_limit,
    weapon,
    armour,
    flask,
    tincture,
    enchant,
    enchant_mod_types,
});
crate::codec_struct!(Affix {
    id,
    typ,
    affix,
    lines,
    stat_order,
    mod_tags,
    weight_key,
    weight_val,
    weight_multiplier_key,
    weight_multiplier_val,
});
crate::codec_struct!(ClusterSkill {
    name,
    stats,
    mastery
});
crate::codec_struct!(ClusterJewel {
    size,
    size_index,
    min_nodes,
    max_nodes,
    skills,
    skill_data,
    small_indicies,
    notable_indicies,
    socket_indicies,
    total_indicies,
});
crate::codec_struct!(Scalability {
    is_scalable,
    formats
});

/// The affixes and affix sets, each written once however many sets and
/// groups share it
impl Codec for ItemData {
    fn put(&self, w: &mut Writer) {
        let mut affixes: Vec<&Rc<Affix>> = Vec::new();
        let mut affix_index: HashMap<*const Affix, usize> = HashMap::new();
        let mut sets: Vec<Rc<AffixSet>> = Vec::new();
        let mut set_index = |s: &Rc<AffixSet>| match sets.iter().position(|x| Rc::ptr_eq(x, s)) {
            Some(i) => i,
            None => {
                sets.push(s.clone());
                sets.len() - 1
            }
        };
        let mut groups: Vec<_> = self.item_mods.iter().collect();
        groups.sort_unstable_by_key(|(k, _)| *k);
        let groups: Vec<(&String, usize)> =
            groups.into_iter().map(|(k, s)| (k, set_index(s))).collect();
        let mut rare: Vec<_> = self.rare_like_uniques.iter().collect();
        rare.sort_unstable_by_key(|(k, _)| *k);
        let rare: Vec<_> = rare
            .into_iter()
            .map(|(k, u)| (k, set_index(&u.affixes), u))
            .collect();
        let sets: Vec<Vec<usize>> = sets
            .iter()
            .map(|s| {
                s.list
                    .iter()
                    .map(|a| {
                        *affix_index.entry(Rc::as_ptr(a)).or_insert_with(|| {
                            affixes.push(a);
                            affixes.len() - 1
                        })
                    })
                    .collect()
            })
            .collect();
        w.uint(affixes.len() as u64);
        for a in affixes {
            a.put(w);
        }
        sets.put(w);
        w.uint(groups.len() as u64);
        for (k, i) in groups {
            k.put(w);
            i.put(w);
        }
        w.uint(rare.len() as u64);
        for (k, i, u) in rare {
            k.put(w);
            i.put(w);
            u.valid_base_tags.put(w);
            u.prefix_limit.put(w);
            u.suffix_limit.put(w);
            u.ignore_mod_type.put(w);
        }
        self.bases.put(w);
        self.foulborn_map.put(w);
        self.cluster_jewels.put(w);
        self.cluster_notable_sort_order.put(w);
        self.cluster_orbit_offsets.put(w);
        self.mod_scalability.put(w);
        self.high_precision_mods.put(w);
        self.item_tag_special.put(w);
        self.item_tag_special_exclusion.put(w);
        self.default_high_precision.put(w);
        self.color_codes.put(w);
        self.caster_tag_crucible_uniques.put(w);
        self.minion_tag_crucible_uniques.put(w);
        self.unique_stat_order_exact.put(w);
        self.unique_stat_order_normalised.put(w);
    }

    fn get(r: &mut Reader) -> codec::Result<Self> {
        let affixes = Vec::<Rc<Affix>>::get(r)?;
        let sets = Vec::<Vec<usize>>::get(r)?
            .into_iter()
            .map(|s| {
                let list = s
                    .into_iter()
                    .map(|i| {
                        affixes
                            .get(i)
                            .cloned()
                            .ok_or_else(|| "bad affix index".into())
                    })
                    .collect::<codec::Result<Vec<_>>>()?;
                let index = list
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (a.id.clone(), i))
                    .collect();
                Ok(Rc::new(AffixSet { list, index }))
            })
            .collect::<codec::Result<Vec<_>>>()?;
        let set = |i: usize| -> codec::Result<_> {
            sets.get(i)
                .cloned()
                .ok_or_else(|| "bad affix set index".into())
        };
        let mut item_mods = HashMap::new();
        for _ in 0..r.count()? {
            let k = String::get(r)?;
            item_mods.insert(k, set(usize::get(r)?)?);
        }
        let mut rare_like_uniques = HashMap::new();
        for _ in 0..r.count()? {
            let k = String::get(r)?;
            let affixes = set(usize::get(r)?)?;
            rare_like_uniques.insert(
                k,
                RareLikeUnique {
                    affixes,
                    valid_base_tags: Codec::get(r)?,
                    prefix_limit: Codec::get(r)?,
                    suffix_limit: Codec::get(r)?,
                    ignore_mod_type: Codec::get(r)?,
                },
            );
        }
        let bases = Vec::<Rc<Base>>::get(r)?;
        let base_index = bases
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        Ok(ItemData {
            bases,
            base_index,
            item_mods,
            rare_like_uniques,
            foulborn_map: Codec::get(r)?,
            cluster_jewels: Codec::get(r)?,
            cluster_notable_sort_order: Codec::get(r)?,
            cluster_orbit_offsets: Codec::get(r)?,
            mod_scalability: Codec::get(r)?,
            high_precision_mods: Codec::get(r)?,
            item_tag_special: Codec::get(r)?,
            item_tag_special_exclusion: Codec::get(r)?,
            default_high_precision: Codec::get(r)?,
            color_codes: Codec::get(r)?,
            caster_tag_crucible_uniques: Codec::get(r)?,
            minion_tag_crucible_uniques: Codec::get(r)?,
            unique_stat_order_exact: Codec::get(r)?,
            unique_stat_order_normalised: Codec::get(r)?,
        })
    }
}

/// A Lua array of numbers as integers
fn ints(v: &Value) -> Result<Vec<i64>> {
    array(v).iter().map(|x| Ok(number(x)? as i64)).collect()
}
