//! Converts upstream's mods as Lua values (the importer's data, the mod
//! parser's output, the translated program's stores) into typed ones.
//! Conversion fails on any field or tag it does not know, so what upstream
//! adds shows up instead of being dropped.

use super::tag::*;
use super::*;
use lua::rt;
use lua::{Key, TableRef, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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

fn flag(v: &Value) -> bool {
    v.truthy()
}

/// `bit.band`'s view of a flags number
fn flags(v: &Value) -> Result<u32, String> {
    let n = number(v)?;
    Ok(n.rem_euclid(4294967296.0) as u64 as u32)
}

/// The values of `t[1..]`
pub fn array(v: &Value) -> Vec<Value> {
    match v {
        Value::Table(t) => t.array(),
        _ => vec![],
    }
}

/// `var`/`varList` style names; `pairs` order for the list
fn names(one: &Value, list: &Value) -> Result<Option<Names>, String> {
    if let Value::Table(t) = list {
        let mut out = Vec::new();
        for (_, v) in t.entries() {
            out.push(sym(&v)?);
        }
        return Ok(Some(Names::List(out)));
    }
    Ok(opt_sym(one)?.map(Names::One))
}

/// The keys of a table other than its array part
fn keys(t: &TableRef) -> Vec<String> {
    t.entries()
        .into_iter()
        .filter_map(|(k, _)| match k {
            Key::Str(s) => Some(s.to_str_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

thread_local! {
    /// With `MODS_SURVEY` set, unknown fields are collected here instead
    pub static UNKNOWN: RefCell<std::collections::BTreeMap<String, usize>> = Default::default();
}

fn check_fields(t: &TableRef, what: &str, known: &[&str]) -> Result<(), String> {
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
}

impl Converter<'_> {
    pub fn value(&self, v: &Value) -> Result<ModValue, String> {
        Ok(match v {
            Value::Bool(b) => ModValue::Bool(*b),
            Value::Number(n) => ModValue::Number(*n),
            Value::Str(x) => ModValue::Str(x.to_str_lossy().into()),
            Value::Func(f) => ModValue::Func(f.key.clone()),
            Value::Function(f) => ModValue::Func(f.name.clone()),
            Value::Closure(c) => ModValue::Func(c.name.clone()),
            Value::Nil => return Err("nil value".into()),
            Value::Table(t) => {
                if !get(v, "name").is_nil() && !get(v, "type").is_nil() && !get(v, "flags").is_nil()
                {
                    return Ok(ModValue::Mod(Box::new(self.mod_(v)?)));
                }
                // `{ ["$num"] = "inf" }`, how the oracle encodes infinities, does
                // not occur in the live program; numbers stay numbers
                let n = t.borrow().len();
                if n > 0 && t.borrow().len_pairs() == n {
                    let mut list = Vec::new();
                    for x in t.array() {
                        list.push(self.value(&x)?);
                    }
                    return Ok(ModValue::List(list));
                }
                let mut rec = Record::default();
                for (k, x) in t.entries() {
                    let name = match k {
                        Key::Str(s) => Sym::new(&s.to_str_lossy()),
                        Key::Int(i) => Sym::new(&i.to_string()),
                        k => return Err(format!("record key {k:?}")),
                    };
                    rec.fields.push((name, self.value(&x)?));
                }
                ModValue::Record(rec)
            }
        })
    }

    pub fn mod_(&self, v: &Value) -> Result<Mod, String> {
        let Value::Table(t) = v else {
            return Err(format!("mod is {v:?}"));
        };
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
        let typ = ModType::new(&text(&get(v, "type"))?);
        let raw = get(v, "value");
        let value = self.value(&raw)?;
        let mut tags = Vec::new();
        for tag in array(v) {
            tags.push(self.tag(&tag)?);
        }
        Ok(Mod {
            name: sym(&get(v, "name"))?,
            typ,
            value,
            flags: flags(&get(v, "flags"))?,
            keyword_flags: flags(&get(v, "keywordFlags"))?,
            source: match get(v, "source") {
                Value::Nil => None,
                x => Some(text(&x)?),
            },
            source_slot: match get(v, "sourceSlot") {
                Value::Nil => None,
                x => Some(text(&x)?),
            },
            tags,
        })
    }

    pub fn tag(&self, v: &Value) -> Result<Tag, String> {
        let Value::Table(t) = v else {
            return Err(format!("tag is {v:?}"));
        };
        let g = |k: &str| get(v, k);
        let kind_name = text(&g("type"))?;
        let global_limit = match (g("globalLimit"), g("globalLimitKey")) {
            (l, k) if l.truthy() && k.truthy() => Some((number(&l)?, sym(&k)?)),
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
                    actor: opt_sym(&g("actor"))?,
                    limit_actor: opt_sym(&g("limitActor"))?,
                    div: opt_number(&g("div"))?,
                    div_var: opt_sym(&g("divVar"))?,
                    no_floor: flag(&g("noFloor")),
                    limit: opt_number(&g("limit"))?,
                    limit_var: opt_sym(&g("limitVar"))?,
                    limit_stat: opt_sym(&g("limitStat"))?,
                    limit_total: flag(&g("limitTotal")),
                    limit_neg_total: flag(&g("limitNegTotal")),
                    invert: flag(&g("invert")),
                    base: opt_number(&g("base"))?,
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
                    actor: opt_sym(&g("actor"))?,
                    threshold_actor: opt_sym(&g("thresholdActor"))?,
                    threshold: opt_number(&g("threshold"))?,
                    threshold_var: opt_sym(&g("thresholdVar"))?,
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
                    actor: opt_sym(&g("actor"))?,
                    div: opt_number(&g("div"))?,
                    div_var: opt_sym(&g("divVar"))?,
                    limit: opt_number(&g("limit"))?,
                    limit_var: opt_sym(&g("limitVar"))?,
                    limit_total: flag(&g("limitTotal")),
                    base: opt_number(&g("base"))?,
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
                    actor: opt_sym(&g("actor"))?,
                    percent: opt_number(&g("percent"))?,
                    percent_var: opt_sym(&g("percentVar"))?,
                    floor: flag(&g("floor")),
                    limit: opt_number(&g("limit"))?,
                    limit_var: opt_sym(&g("limitVar"))?,
                    limit_total: flag(&g("limitTotal")),
                    base: opt_number(&g("base"))?,
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
                    threshold: opt_number(&g("threshold"))?,
                    threshold_stat: opt_sym(&g("thresholdStat"))?,
                    threshold_percent: opt_number(&g("thresholdPercent"))?,
                    threshold_percent_var: opt_sym(&g("thresholdPercentVar"))?,
                    upper: flag(&g("upper")),
                })
            }
            "DistanceRamp" => {
                check(&["ramp"])?;
                let mut ramp = Vec::new();
                for p in array(&g("ramp")) {
                    ramp.push((number(&get_i(&p, 1))?, number(&get_i(&p, 2))?));
                }
                TagKind::DistanceRamp(ramp)
            }
            "MeleeProximity" => {
                check(&["ramp"])?;
                TagKind::MeleeProximity(
                    array(&g("ramp"))
                        .iter()
                        .map(number)
                        .collect::<Result<_, _>>()?,
                )
            }
            "Limit" => {
                check(&["limit", "limitVar"])?;
                TagKind::Limit {
                    limit: opt_number(&g("limit"))?,
                    limit_var: opt_sym(&g("limitVar"))?,
                }
            }
            "Condition" => {
                check(&["var", "varList", "neg"])?;
                TagKind::Condition {
                    vars: names(&g("var"), &g("varList"))?.ok_or("Condition tag without var")?,
                    neg: flag(&g("neg")),
                }
            }
            "ActorCondition" => {
                check(&["actor", "var", "varList", "neg"])?;
                TagKind::ActorCondition {
                    actor: opt_sym(&g("actor"))?,
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
                    Value::Nil => Ok(None),
                    x => text(&x).map(Some),
                };
                let opt_bool = |k: &str| match g(k) {
                    Value::Nil => None,
                    x => Some(x.truthy()),
                };
                let ic = Rc::new(ItemCondition {
                    item_slot: text(&g("itemSlot"))?,
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
                if let Some(o) = self.originals {
                    o.borrow_mut()
                        .item_conditions
                        .insert(Rc::as_ptr(&ic), v.clone());
                }
                TagKind::ItemCondition(ic)
            }
            "SocketedIn" => {
                check(&["slotName", "keyword", "socketColor", "sockets", "neg"])?;
                let sockets = match g("sockets") {
                    Value::Nil => None,
                    Value::Str(x) if x.as_bytes() == b"all" => Some(Sockets::All),
                    Value::Number(n) => Some(Sockets::Fewer(n)),
                    Value::Table(t) => {
                        let mut nums = Vec::new();
                        for x in t.array() {
                            nums.push(number(&x)?);
                        }
                        Some(Sockets::Numbers(nums))
                    }
                    x => return Err(format!("SocketedIn sockets {x:?}")),
                };
                let opt_text = |k: &str| match g(k) {
                    Value::Nil => Ok(None),
                    x => text(&x).map(Some),
                };
                TagKind::SocketedIn(Rc::new(SocketedIn {
                    slot_name: opt_sym(&g("slotName"))?,
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
                        Value::Nil => None,
                        x => Some(text(&x)?),
                    },
                }
            }
            "SkillId" => {
                check(&["skillId"])?;
                TagKind::SkillId(match g("skillId") {
                    Value::Nil => None,
                    x => Some(text(&x)?),
                })
            }
            "SkillPart" => {
                check(&["skillPart", "skillPartList", "neg"])?;
                TagKind::SkillPart {
                    list: number_list(&g("skillPartList"))?,
                    part: opt_number(&g("skillPart"))?,
                    neg: flag(&g("neg")),
                }
            }
            "SkillType" => {
                check(&["skillType", "skillTypeList", "neg"])?;
                TagKind::SkillType {
                    list: number_list_pairs(&g("skillTypeList"))?,
                    skill_type: opt_number(&g("skillType"))?,
                    neg: flag(&g("neg")),
                }
            }
            "BaseFlag" => {
                check(&["baseFlag", "neg"])?;
                TagKind::BaseFlag {
                    flag: sym(&g("baseFlag"))?,
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
                TagKind::ModFlagOr(flags(&g("modFlags"))?)
            }
            "KeywordFlagAnd" => {
                check(&["keywordFlags"])?;
                TagKind::KeywordFlagAnd(flags(&g("keywordFlags"))?)
            }
            "MonsterTag" => {
                check(&["monsterTag", "monsterTagList", "neg"])?;
                TagKind::MonsterTag {
                    names: names(&g("monsterTag"), &g("monsterTagList"))?,
                    neg: flag(&g("neg")),
                }
            }
            other => {
                let Value::Table(_) = v else { unreachable!() };
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

fn get_i(v: &Value, i: i64) -> Value {
    rt::index(v, &Value::Number(i as f64)).unwrap_or_default()
}

/// `stat`/`statList`; `ipairs` order for the list
fn stat_names(one: &Value, list: &Value) -> Result<Option<Names>, String> {
    if list.is_nil() {
        return Ok(opt_sym(one)?.map(Names::One));
    }
    let mut out = Vec::new();
    for v in array(list) {
        out.push(sym(&v)?);
    }
    Ok(Some(Names::List(out)))
}

fn number_list(v: &Value) -> Result<Option<Vec<f64>>, String> {
    if v.is_nil() {
        return Ok(None);
    }
    array(v)
        .iter()
        .map(number)
        .collect::<Result<_, _>>()
        .map(Some)
}

/// A list `pairs` visits
fn number_list_pairs(v: &Value) -> Result<Option<Vec<f64>>, String> {
    let Value::Table(t) = v else {
        return Ok(None);
    };
    t.entries()
        .iter()
        .map(|(_, x)| number(x))
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
