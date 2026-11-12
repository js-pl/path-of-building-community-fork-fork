//! Converts upstream's mods as Lua values (the importer's data, the mod
//! parser's output, the translated program's stores) into typed ones.
//! Conversion fails on any field or tag it does not know, so what upstream
//! adds shows up instead of being dropped.

use super::tag::*;
use super::*;
use lua::rt;
use lua::{Key, Value};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A table key as conversion sees it
pub enum DKey {
    Str(String),
    Int(i64),
    Other(String),
}

/// What a dynamic value is
pub enum Kind<'a, D> {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Cow<'a, str>),
    Table,
    /// A closure made by ported code: its name and upvalues
    Closure(Rc<str>, Vec<D>),
    /// `getThreshold`'s closure: its attribute and the mod it builds
    Threshold(D, D),
    /// Already typed
    Typed(ModValue),
}

/// Dynamic values upstream's mods come as: Lua values (the importer's data,
/// the translated program) and the mod parser's closure values
pub trait Dyn: Clone {
    fn kind(&self) -> Kind<'_, Self>;
    /// `t.k`, nil for non-tables
    fn get(&self, k: &str) -> Self;
    /// `t[i]`
    fn get_i(&self, i: i64) -> Self;
    /// `#t`
    fn border(&self) -> usize;
    /// Non-nil entries
    fn len_pairs(&self) -> usize;
    /// Entries in `pairs` order
    fn entries(&self) -> Vec<(DKey, Self)>;
    /// The Lua value, kept for tags whose evaluation needs it
    fn lua(&self) -> Option<&Value> {
        None
    }

    fn is_nil(&self) -> bool {
        matches!(self.kind(), Kind::Nil)
    }

    fn truthy(&self) -> bool {
        !matches!(self.kind(), Kind::Nil | Kind::Bool(false))
    }

    fn is_table(&self) -> bool {
        matches!(self.kind(), Kind::Table)
    }
}

impl Dyn for Value {
    fn kind(&self) -> Kind<'_, Value> {
        match self {
            Value::Nil => Kind::Nil,
            Value::Bool(b) => Kind::Bool(*b),
            Value::Number(n) => Kind::Num(*n),
            Value::Str(x) => Kind::Str(x.to_str_lossy()),
            Value::Table(_) => Kind::Table,
            Value::Func(f) => Kind::Typed(ModValue::Func(Rc::new(Func {
                name: f.key.clone(),
                args: vec![],
            }))),
            Value::Function(f) => Kind::Typed(ModValue::Func(Rc::new(Func {
                name: f.name.clone(),
                args: vec![],
            }))),
            // `getThreshold` closes over the mod it builds,
            // `mod(name, type, value, "", ...)`
            Value::Closure(c) if &*c.name == "getThreshold" => {
                let attrib = c.upvalues.first().cloned().unwrap_or_default();
                let mut mod_args = c
                    .upvalues
                    .get(1..4)
                    .map(<[Value]>::to_vec)
                    .unwrap_or_default();
                mod_args.push(Value::str(""));
                mod_args.extend(c.upvalues.iter().skip(4).cloned());
                let base = lua::modlib::create_mod_args(&mod_args);
                Kind::Threshold(attrib, Value::Table(base))
            }
            Value::Closure(c) => Kind::Closure(c.name.clone(), c.upvalues.clone()),
        }
    }

    fn get(&self, k: &str) -> Value {
        match self {
            Value::Table(t) => t.get_str(k),
            _ => Value::Nil,
        }
    }

    fn get_i(&self, i: i64) -> Value {
        match self {
            Value::Table(t) => t.get_int(i),
            _ => Value::Nil,
        }
    }

    fn border(&self) -> usize {
        match self {
            Value::Table(t) => t.borrow().len(),
            _ => 0,
        }
    }

    fn len_pairs(&self) -> usize {
        match self {
            Value::Table(t) => t.borrow().len_pairs(),
            _ => 0,
        }
    }

    fn entries(&self) -> Vec<(DKey, Value)> {
        match self {
            Value::Table(t) => t
                .entries()
                .into_iter()
                .map(|(k, v)| {
                    let k = match k {
                        Key::Str(s) => DKey::Str(s.to_str_lossy().into_owned()),
                        Key::Int(i) => DKey::Int(i),
                        k => DKey::Other(format!("{k:?}")),
                    };
                    (k, v)
                })
                .collect(),
            _ => vec![],
        }
    }

    fn lua(&self) -> Option<&Value> {
        Some(self)
    }
}

fn d_text<D: Dyn>(v: &D) -> Result<Rc<str>, String> {
    match v.kind() {
        Kind::Str(x) => Ok(x.into()),
        _ => Err(format!("expected a string, got {}", describe(v))),
    }
}

fn describe<D: Dyn>(v: &D) -> String {
    match v.kind() {
        Kind::Nil => "nil".into(),
        Kind::Bool(b) => format!("{b}"),
        Kind::Num(n) => format!("{n}"),
        Kind::Str(s) => format!("{s:?}"),
        Kind::Table => "a table".into(),
        _ => "a function".into(),
    }
}

fn d_sym<D: Dyn>(v: &D) -> Result<Sym, String> {
    Ok(Sym::new(&d_text(v)?))
}

fn d_opt_sym<D: Dyn>(v: &D) -> Result<Option<Sym>, String> {
    if v.is_nil() {
        Ok(None)
    } else {
        d_sym(v).map(Some)
    }
}

fn d_number<D: Dyn>(v: &D) -> Result<f64, String> {
    match v.kind() {
        Kind::Num(n) => Ok(n),
        Kind::Str(s) => {
            lua::str_to_number(s.as_bytes()).ok_or_else(|| format!("expected a number, got {s:?}"))
        }
        _ => Err(format!("expected a number, got {}", describe(v))),
    }
}

fn d_opt_number<D: Dyn>(v: &D) -> Result<Option<f64>, String> {
    if v.is_nil() {
        Ok(None)
    } else {
        d_number(v).map(Some)
    }
}

fn d_flags<D: Dyn>(v: &D) -> Result<u32, String> {
    let n = d_number(v)?;
    Ok(n.rem_euclid(4294967296.0) as u64 as u32)
}

/// `t[1..#t]`
pub fn d_array<D: Dyn>(v: &D) -> Vec<D> {
    if !v.is_table() {
        return vec![];
    }
    (1..=v.border() as i64).map(|i| v.get_i(i)).collect()
}

pub fn s(x: &str) -> Value {
    Value::str(x)
}

pub fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

pub fn text(v: &Value) -> Result<Rc<str>, String> {
    match v {
        Value::Str(x) => Ok(x.to_str_lossy().into()),
        v => Err(format!("expected a string, got {v:?}")),
    }
}

pub fn sym(v: &Value) -> Result<Sym, String> {
    Ok(Sym::new(&text(v)?))
}

pub fn opt_sym(v: &Value) -> Result<Option<Sym>, String> {
    if v.is_nil() {
        Ok(None)
    } else {
        sym(v).map(Some)
    }
}

/// A number, or a string Lua's arithmetic would coerce
pub fn number(v: &Value) -> Result<f64, String> {
    lua::to_number(v).ok_or_else(|| format!("expected a number, got {v:?}"))
}

pub fn opt_number(v: &Value) -> Result<Option<f64>, String> {
    if v.is_nil() {
        Ok(None)
    } else {
        number(v).map(Some)
    }
}

fn flag<D: Dyn>(v: &D) -> bool {
    v.truthy()
}

/// The values of `t[1..]`
pub fn array(v: &Value) -> Vec<Value> {
    match v {
        Value::Table(t) => t.array(),
        _ => vec![],
    }
}

/// `var`/`varList` style names; `pairs` order for the list
fn names<D: Dyn>(one: &D, list: &D) -> Result<Option<Names>, String> {
    if list.is_table() {
        let mut out = Vec::new();
        for (_, v) in list.entries() {
            out.push(d_sym(&v)?);
        }
        return Ok(Some(Names::List(out)));
    }
    Ok(d_opt_sym(one)?.map(Names::One))
}

/// The keys of a table other than its array part
fn keys<D: Dyn>(t: &D) -> Vec<String> {
    t.entries()
        .into_iter()
        .filter_map(|(k, _)| match k {
            DKey::Str(s) => Some(s),
            _ => None,
        })
        .collect()
}

thread_local! {
    /// With `MODS_SURVEY` set, unknown fields are collected here instead
    pub static UNKNOWN: RefCell<std::collections::BTreeMap<String, usize>> = Default::default();
}

pub fn check_fields<D: Dyn>(t: &D, what: &str, known: &[&str]) -> Result<(), String> {
    for k in keys(t) {
        if !known.contains(&k.as_str()) {
            if std::env::var_os("MODS_SURVEY").is_some() {
                UNKNOWN.with(|u| *u.borrow_mut().entry(format!("{what}: {k}")).or_default() += 1);
                continue;
            }
            return Err(format!("{what}: unknown field '{k}'"));
        }
    }
    Ok(())
}

/// Lua values of tags whose evaluation needs the translated program
#[derive(Default)]
pub struct Originals {
    pub item_conditions: HashMap<*const ItemCondition, Value>,
}

/// Converts mods, tags and values. With `originals`, the Lua value of
/// every `ItemCondition` tag is kept there.
#[derive(Default)]
pub struct Converter<'o> {
    pub originals: Option<&'o RefCell<Originals>>,
    /// Mods as templates, whose value may be nil until it is set (the mods
    /// of the skills' data); a nil value converts to NaN
    pub templates: bool,
}

impl Converter<'_> {
    /// A mod's flags. A few of upstream's skill mods pass a tag where the
    /// flags go; upstream fails on the queries of such a mod, and as
    /// templates they take flags no query has, so queries skip them.
    fn mod_flags<D: Dyn>(&self, v: &D) -> Result<u32, String> {
        if self.templates && v.is_table() {
            return Ok(u32::MAX);
        }
        d_flags(v)
    }

    pub fn value<D: Dyn>(&self, v: &D) -> Result<ModValue, String> {
        Ok(match v.kind() {
            Kind::Bool(b) => ModValue::Bool(b),
            Kind::Num(n) => ModValue::Number(n),
            Kind::Str(x) => ModValue::Str(x.into()),
            Kind::Typed(m) => m,
            Kind::Threshold(attrib, base) => ModValue::Func(Rc::new(Func {
                name: "getThreshold".into(),
                args: vec![
                    Some(self.value(&attrib)?),
                    Some(ModValue::Mod(Box::new(self.mod_(&base)?))),
                ],
            })),
            Kind::Closure(name, upvalues) => ModValue::Func(Rc::new(Func {
                name,
                args: upvalues
                    .iter()
                    .map(|u| {
                        if u.is_nil() {
                            Ok(None)
                        } else {
                            self.value(u).map(Some)
                        }
                    })
                    .collect::<Result<_, String>>()?,
            })),
            Kind::Nil => return Err("nil value".into()),
            Kind::Table => {
                if !v.get("name").is_nil() && !v.get("type").is_nil() && !v.get("flags").is_nil() {
                    return Ok(ModValue::Mod(Box::new(self.mod_(v)?)));
                }
                // `{ ["$num"] = "inf" }`, how the oracle encodes infinities, does
                // not occur in the live program; numbers stay numbers
                let n = v.border();
                if n > 0 && v.len_pairs() == n {
                    let mut list = Vec::new();
                    for x in d_array(v) {
                        list.push(self.value(&x)?);
                    }
                    return Ok(ModValue::List(list));
                }
                let mut rec = Record::default();
                for (k, x) in v.entries() {
                    let name = match k {
                        DKey::Str(s) => Sym::new(&s),
                        DKey::Int(i) => Sym::new(&i.to_string()),
                        DKey::Other(k) => return Err(format!("record key {k}")),
                    };
                    rec.fields.push((name, self.value(&x)?));
                }
                ModValue::Record(rec)
            }
        })
    }

    pub fn mod_<D: Dyn>(&self, v: &D) -> Result<Mod, String> {
        if let Kind::Typed(ModValue::Mod(m)) = v.kind() {
            return Ok(*m);
        }
        if !v.is_table() {
            return Err(format!("mod is {}", describe(v)));
        }
        let t = v;
        check_fields(
            t,
            "mod",
            &[
                "name",
                "type",
                "value",
                "flags",
                "keywordFlags",
                "source",
                "sourceSlot",
                "replaced",
                "converted",
            ],
        )?;
        let typ = ModType::new(&d_text(&v.get("type"))?);
        let raw = v.get("value");
        let value = if self.templates && raw.is_nil() {
            ModValue::Number(f64::NAN)
        } else {
            self.value(&raw)?
        };
        // Upstream visits tags with `ipairs`
        let mut tags = Vec::new();
        loop {
            let tag = t.get_i(tags.len() as i64 + 1);
            if tag.is_nil() {
                break;
            }
            tags.push(self.tag(&tag)?);
        }
        let mut unreached_tags = Vec::new();
        for (k, tag) in t.entries() {
            if let DKey::Int(i) = k
                && i as usize > tags.len() + 1
            {
                unreached_tags.push((i as usize, self.tag(&tag)?));
            }
        }
        Ok(Mod {
            name: d_sym(&v.get("name"))?,
            typ,
            value,
            flags: self.mod_flags(&v.get("flags"))?,
            keyword_flags: self.mod_flags(&v.get("keywordFlags"))?,
            source: match v.get("source") {
                x if x.is_nil() => None,
                x => Some(d_text(&x)?),
            },
            source_slot: match v.get("sourceSlot") {
                x if x.is_nil() => None,
                x => Some(d_text(&x)?),
            },
            tags: tags.into(),
            unreached_tags,
        })
    }

    pub fn tag<D: Dyn>(&self, v: &D) -> Result<Tag, String> {
        if !v.is_table() {
            return Err(format!("tag is {}", describe(v)));
        }
        let t = v;
        let g = |k: &str| v.get(k);
        let kind_name = d_text(&g("type"))?;
        let global_limit = match (g("globalLimit"), g("globalLimitKey")) {
            (l, k) if l.truthy() && k.truthy() => Some((d_number(&l)?, d_sym(&k)?)),
            _ => None,
        };
        let check = |known: &[&str]| {
            let mut all = vec!["type", "globalLimit", "globalLimitKey"];
            all.extend_from_slice(known);
            check_fields(t, &format!("{kind_name} tag"), &all)
        };
        let kind = match &*kind_name {
            "Multiplier" => {
                check(&[
                    "var",
                    "varList",
                    "actor",
                    "limitActor",
                    "div",
                    "divVar",
                    "noFloor",
                    "limit",
                    "limitVar",
                    "limitStat",
                    "limitTotal",
                    "limitNegTotal",
                    "invert",
                    "base",
                ])?;
                TagKind::Multiplier(Multiplier {
                    vars: names(&g("var"), &g("varList"))?,
                    actor: d_opt_sym(&g("actor"))?,
                    limit_actor: d_opt_sym(&g("limitActor"))?,
                    div: d_opt_number(&g("div"))?,
                    div_var: d_opt_sym(&g("divVar"))?,
                    no_floor: flag(&g("noFloor")),
                    limit: d_opt_number(&g("limit"))?,
                    limit_var: d_opt_sym(&g("limitVar"))?,
                    limit_stat: d_opt_sym(&g("limitStat"))?,
                    limit_total: flag(&g("limitTotal")),
                    limit_neg_total: flag(&g("limitNegTotal")),
                    invert: flag(&g("invert")),
                    base: d_opt_number(&g("base"))?,
                })
            }
            "MultiplierThreshold" => {
                check(&[
                    "var",
                    "varList",
                    "actor",
                    "thresholdActor",
                    "threshold",
                    "thresholdVar",
                    "upper",
                    "equals",
                ])?;
                TagKind::MultiplierThreshold(MultiplierThreshold {
                    vars: names(&g("var"), &g("varList"))?,
                    actor: d_opt_sym(&g("actor"))?,
                    threshold_actor: d_opt_sym(&g("thresholdActor"))?,
                    threshold: d_opt_number(&g("threshold"))?,
                    threshold_var: d_opt_sym(&g("thresholdVar"))?,
                    upper: flag(&g("upper")),
                    equals: flag(&g("equals")),
                })
            }
            "PerStat" => {
                check(&[
                    "stat",
                    "statList",
                    "actor",
                    "div",
                    "divVar",
                    "limit",
                    "limitVar",
                    "limitTotal",
                    "base",
                ])?;
                TagKind::PerStat(PerStat {
                    stats: stat_names(&g("stat"), &g("statList"))?,
                    actor: d_opt_sym(&g("actor"))?,
                    div: d_opt_number(&g("div"))?,
                    div_var: d_opt_sym(&g("divVar"))?,
                    limit: d_opt_number(&g("limit"))?,
                    limit_var: d_opt_sym(&g("limitVar"))?,
                    limit_total: flag(&g("limitTotal")),
                    base: d_opt_number(&g("base"))?,
                })
            }
            "PercentStat" => {
                check(&[
                    "stat",
                    "statList",
                    "actor",
                    "percent",
                    "percentVar",
                    "floor",
                    "limit",
                    "limitVar",
                    "limitTotal",
                    "base",
                ])?;
                TagKind::PercentStat(PercentStat {
                    stats: stat_names(&g("stat"), &g("statList"))?,
                    actor: d_opt_sym(&g("actor"))?,
                    percent: d_opt_number(&g("percent"))?,
                    percent_var: d_opt_sym(&g("percentVar"))?,
                    floor: flag(&g("floor")),
                    limit: d_opt_number(&g("limit"))?,
                    limit_var: d_opt_sym(&g("limitVar"))?,
                    limit_total: flag(&g("limitTotal")),
                    base: d_opt_number(&g("base"))?,
                })
            }
            "StatThreshold" => {
                check(&[
                    "stat",
                    "statList",
                    "threshold",
                    "thresholdStat",
                    "thresholdPercent",
                    "thresholdPercentVar",
                    "upper",
                ])?;
                TagKind::StatThreshold(StatThreshold {
                    stats: stat_names(&g("stat"), &g("statList"))?,
                    threshold: d_opt_number(&g("threshold"))?,
                    threshold_stat: d_opt_sym(&g("thresholdStat"))?,
                    threshold_percent: d_opt_number(&g("thresholdPercent"))?,
                    threshold_percent_var: d_opt_sym(&g("thresholdPercentVar"))?,
                    upper: flag(&g("upper")),
                })
            }
            "DistanceRamp" => {
                check(&["ramp"])?;
                let mut ramp = Vec::new();
                for p in d_array(&g("ramp")) {
                    ramp.push((d_number(&p.get_i(1))?, d_number(&p.get_i(2))?));
                }
                TagKind::DistanceRamp(ramp)
            }
            "MeleeProximity" => {
                check(&["ramp"])?;
                TagKind::MeleeProximity(
                    d_array(&g("ramp"))
                        .iter()
                        .map(d_number)
                        .collect::<Result<_, _>>()?,
                )
            }
            "Limit" => {
                check(&["limit", "limitVar"])?;
                TagKind::Limit {
                    limit: d_opt_number(&g("limit"))?,
                    limit_var: d_opt_sym(&g("limitVar"))?,
                }
            }
            "Condition" => {
                check(&["var", "varList", "neg", "actor"])?;
                TagKind::Condition {
                    vars: names(&g("var"), &g("varList"))?.ok_or("Condition tag without var")?,
                    neg: flag(&g("neg")),
                    actor: d_opt_sym(&g("actor"))?,
                }
            }
            "ActorCondition" => {
                check(&["actor", "var", "varList", "neg"])?;
                TagKind::ActorCondition {
                    actor: d_opt_sym(&g("actor"))?,
                    vars: names(&g("var"), &g("varList"))?,
                    neg: flag(&g("neg")),
                }
            }
            "ItemCondition" => {
                check(&[
                    "itemSlot",
                    "allSlots",
                    "bothSlots",
                    "excludeSelf",
                    "searchCond",
                    "rarityCond",
                    "corruptedCond",
                    "shaperCond",
                    "elderCond",
                    "nameCond",
                    "neg",
                ])?;
                let opt_text = |k: &str| match g(k) {
                    x if x.is_nil() => Ok(None),
                    x => d_text(&x).map(Some),
                };
                let opt_bool = |k: &str| match g(k) {
                    x if x.is_nil() => None,
                    x => Some(x.truthy()),
                };
                let ic = Rc::new(ItemCondition {
                    item_slot: d_text(&g("itemSlot"))?,
                    all_slots: flag(&g("allSlots")),
                    both_slots: flag(&g("bothSlots")),
                    exclude_self: flag(&g("excludeSelf")),
                    search_cond: opt_text("searchCond")?,
                    rarity_cond: opt_text("rarityCond")?,
                    corrupted_cond: opt_bool("corruptedCond"),
                    shaper_cond: opt_bool("shaperCond"),
                    elder_cond: opt_bool("elderCond"),
                    name_cond: opt_text("nameCond")?,
                    neg: flag(&g("neg")),
                });
                if let (Some(o), Some(lv)) = (self.originals, v.lua()) {
                    o.borrow_mut()
                        .item_conditions
                        .insert(Rc::as_ptr(&ic), lv.clone());
                }
                TagKind::ItemCondition(ic)
            }
            "SocketedIn" => {
                check(&["slotName", "keyword", "socketColor", "sockets", "neg"])?;
                let so = g("sockets");
                let sockets = match so.kind() {
                    Kind::Nil => None,
                    Kind::Str(x) if x == "all" => Some(Sockets::All),
                    Kind::Num(n) => Some(Sockets::Fewer(n)),
                    Kind::Table => {
                        let mut nums = Vec::new();
                        for x in d_array(&so) {
                            nums.push(d_number(&x)?);
                        }
                        Some(Sockets::Numbers(nums))
                    }
                    _ => return Err(format!("SocketedIn sockets {}", describe(&so))),
                };
                let opt_text = |k: &str| match g(k) {
                    x if x.is_nil() => Ok(None),
                    x => d_text(&x).map(Some),
                };
                TagKind::SocketedIn(Rc::new(SocketedIn {
                    slot_name: d_opt_sym(&g("slotName"))?,
                    keyword: opt_text("keyword")?,
                    socket_color: opt_text("socketColor")?,
                    sockets,
                    neg: flag(&g("neg")),
                }))
            }
            "SkillName" => {
                check(&[
                    "skillName",
                    "skillNameList",
                    "includeTransfigured",
                    "summonSkill",
                    "neg",
                    "skillId",
                ])?;
                TagKind::SkillName {
                    names: names(&g("skillName"), &g("skillNameList"))?,
                    include_transfigured: flag(&g("includeTransfigured")),
                    summon_skill: flag(&g("summonSkill")),
                    neg: flag(&g("neg")),
                    skill_id: match g("skillId") {
                        x if x.is_nil() => None,
                        x => Some(d_text(&x)?),
                    },
                }
            }
            "SkillId" => {
                check(&["skillId"])?;
                TagKind::SkillId(match g("skillId") {
                    x if x.is_nil() => None,
                    x => Some(d_text(&x)?),
                })
            }
            "SkillPart" => {
                check(&["skillPart", "skillPartList", "neg"])?;
                TagKind::SkillPart {
                    list: number_list(&g("skillPartList"))?,
                    part: d_opt_number(&g("skillPart"))?,
                    neg: flag(&g("neg")),
                }
            }
            "SkillType" => {
                check(&["skillType", "skillTypeList", "neg"])?;
                TagKind::SkillType {
                    list: number_list_pairs(&g("skillTypeList"))?,
                    skill_type: d_opt_number(&g("skillType"))?,
                    neg: flag(&g("neg")),
                }
            }
            "BaseFlag" => {
                check(&["baseFlag", "neg"])?;
                TagKind::BaseFlag {
                    flag: d_sym(&g("baseFlag"))?,
                    neg: flag(&g("neg")),
                }
            }
            "SlotName" => {
                check(&["slotName", "slotNameList", "neg"])?;
                TagKind::SlotName {
                    names: names(&g("slotName"), &g("slotNameList"))?,
                    neg: flag(&g("neg")),
                }
            }
            "ModFlagOr" => {
                check(&["modFlags"])?;
                TagKind::ModFlagOr(d_flags(&g("modFlags"))?)
            }
            "KeywordFlagAnd" => {
                check(&["keywordFlags"])?;
                TagKind::KeywordFlagAnd(d_flags(&g("keywordFlags"))?)
            }
            "MonsterTag" => {
                check(&["monsterTag", "monsterTagList", "neg"])?;
                TagKind::MonsterTag {
                    names: names(&g("monsterTag"), &g("monsterTagList"))?,
                    neg: flag(&g("neg")),
                }
            }
            other => {
                let ModValue::Record(mut fields) = self.value(v)? else {
                    return Err(format!("{other} tag is not a record"));
                };
                fields.fields.retain(|(k, _)| k.as_str() != "type");
                TagKind::Other {
                    kind: Sym::new(other),
                    fields,
                }
            }
        };
        Ok(Tag { kind, global_limit })
    }
}

/// `stat`/`statList`; `ipairs` order for the list
fn stat_names<D: Dyn>(one: &D, list: &D) -> Result<Option<Names>, String> {
    if list.is_nil() {
        return Ok(d_opt_sym(one)?.map(Names::One));
    }
    let mut out = Vec::new();
    for v in d_array(list) {
        out.push(d_sym(&v)?);
    }
    Ok(Some(Names::List(out)))
}

fn number_list<D: Dyn>(v: &D) -> Result<Option<Vec<f64>>, String> {
    if v.is_nil() {
        return Ok(None);
    }
    d_array(v)
        .iter()
        .map(d_number)
        .collect::<Result<_, _>>()
        .map(Some)
}

/// A list `pairs` visits
fn number_list_pairs<D: Dyn>(v: &D) -> Result<Option<Vec<f64>>, String> {
    if !v.is_table() {
        return Ok(None);
    }
    v.entries()
        .iter()
        .map(|(_, x)| d_number(x))
        .collect::<Result<_, _>>()
        .map(Some)
}

/// The truthy keys of `t`, following `__index` tables
pub fn truthy_keys(v: &Value) -> Result<HashSet<Sym>, String> {
    let mut out = HashSet::new();
    let mut cur = v.clone();
    let mut seen_false = HashSet::new();
    while let Value::Table(t) = cur {
        for (k, x) in t.entries() {
            let name = match &k {
                Key::Str(s) => Sym::new(&s.to_str_lossy()),
                _ => continue,
            };
            // A key the nearer table sets decides
            if seen_false.contains(&name) || out.contains(&name) {
                continue;
            }
            if x.truthy() {
                out.insert(name);
            } else {
                seen_false.insert(name);
            }
        }
        let meta = t.borrow().meta.clone();
        cur = match meta {
            Some(m) => match m.get_str("__index") {
                Value::Table(i) => Value::Table(i),
                Value::Nil => Value::Nil,
                _ => return Err("skillCond with an __index function".into()),
            },
            None => Value::Nil,
        };
    }
    Ok(out)
}
