//! Converts the translated program's mod stores into the engine's typed
//! ones (`engine::mods`), to test the typed queries against the translated
//! ones on the same state. Temporary: it goes with the translation.
//!
//! Conversion fails loudly on anything it does not know, so a new field or
//! tag upstream adds shows up here instead of being dropped silently.

use engine::mods::tag::*;
use engine::mods::*;
use lua::rt::{self, first};
use lua::{Key, LResult, LuaStr, TableRef, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn text(v: &Value) -> Result<Rc<str>, String> {
    match v {
        Value::Str(x) => Ok(x.to_str_lossy().into()),
        v => Err(format!("expected a string, got {v:?}")),
    }
}

fn sym(v: &Value) -> Result<Sym, String> {
    Ok(Sym::new(&text(v)?))
}

fn opt_sym(v: &Value) -> Result<Option<Sym>, String> {
    if v.is_nil() {
        Ok(None)
    } else {
        sym(v).map(Some)
    }
}

/// A number, or a string Lua's arithmetic would coerce
fn number(v: &Value) -> Result<f64, String> {
    lua::to_number(v).ok_or_else(|| format!("expected a number, got {v:?}"))
}

fn opt_number(v: &Value) -> Result<Option<f64>, String> {
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
fn array(v: &Value) -> Vec<Value> {
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

fn check_fields(t: &TableRef, what: &str, known: &[&str]) -> Result<(), String> {
    for k in keys(t) {
        if !known.contains(&k.as_str()) {
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

/// Converts mods, tags and values.
pub struct Converter<'o> {
    pub originals: &'o RefCell<Originals>,
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
                "replaced",
                "converted",
            ],
        )?;
        let typ = ModType::new(&text(&get(v, "type"))?);
        let raw = get(v, "value");
        let value = match (&raw, typ) {
            // Numeric strings: arithmetic coerces them, so they are numbers
            (Value::Str(_), ModType::Base | ModType::Inc | ModType::More | ModType::Override) => {
                ModValue::Number(number(&raw)?)
            }
            _ => self.value(&raw)?,
        };
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
                TagKind::MeleeProximity(number(&get_i(&g("ramp"), 1))?)
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
                self.originals
                    .borrow_mut()
                    .item_conditions
                    .insert(Rc::as_ptr(&ic), v.clone());
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
fn truthy_keys(v: &Value) -> Result<HashSet<Sym>, String> {
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

/// Converts a cfg table.
pub fn cfg(v: &Value) -> Result<Option<Cfg>, String> {
    if !v.truthy() {
        return Ok(None);
    }
    let g = |k: &str| get(v, k);
    let opt_text = |k: &str| match g(k) {
        Value::Nil => Ok(None),
        x => text(&x).map(Some),
    };
    let ge = g("skillGrantedEffect");
    let base_flags = {
        let bf = if ge.truthy() {
            get(&ge, "baseFlags")
        } else {
            Value::Nil
        };
        let bf = if bf.truthy() { bf } else { g("baseFlags") };
        if bf.truthy() {
            Some(truthy_keys(&bf)?)
        } else {
            None
        }
    };
    let skill_types = match g("skillTypes") {
        Value::Table(t) => {
            let mut set = HashSet::new();
            for (k, x) in t.entries() {
                if let Key::Int(i) = k
                    && x.truthy()
                {
                    set.insert(i);
                }
            }
            Some(set)
        }
        _ => None,
    };
    let skill_stats = match g("skillStats") {
        Value::Table(t) => {
            let mut map = HashMap::new();
            for (k, x) in t.entries() {
                if let (Key::Str(name), Some(n)) = (k, x.as_number()) {
                    map.insert(Sym::new(&name.to_str_lossy()), n);
                }
            }
            Some(map)
        }
        _ => None,
    };
    Ok(Some(Cfg {
        flags: opt_number(&g("flags"))?.map(|n| n.rem_euclid(4294967296.0) as u64 as u32),
        keyword_flags: opt_number(&g("keywordFlags"))?
            .map(|n| n.rem_euclid(4294967296.0) as u64 as u32),
        source: opt_text("source")?,
        skill_name: opt_text("skillName")?,
        summon_skill_name: opt_text("summonSkillName")?,
        skill_id: if ge.truthy() {
            Some(match get(&ge, "id") {
                Value::Nil => None,
                x => Some(text(&x)?),
            })
        } else {
            None
        },
        base_flags,
        skill_part: opt_number(&g("skillPart"))?,
        skill_types,
        skill_cond: match g("skillCond") {
            Value::Nil => None,
            sc => Some(truthy_keys(&sc)?),
        },
        skill_dist: opt_number(&g("skillDist"))?,
        slot_name: opt_sym(&g("slotName"))?,
        actor: opt_sym(&g("actor"))?,
        skill_stats,
        socket_color: opt_text("socketColor")?,
        socket_num: opt_number(&g("socketNum"))?,
        strength_gems: opt_number(&g("strengthGems"))?,
        dexterity_gems: opt_number(&g("dexterityGems"))?,
        intelligence_gems: opt_number(&g("intelligenceGems"))?,
    }))
}

/// The typed copy of the translated program's stores, and the translated
/// program behind it for what the typed core asks of [`Actors`].
pub struct Bridge {
    pub stores: Stores,
    /// The Lua store of every typed one
    pub lua: Vec<Value>,
    ids: HashMap<*const u8, StoreId>,
    pub originals: RefCell<Originals>,
    /// The Lua cfg of the query running, for what needs upstream's gem data
    pub lua_cfg: RefCell<Value>,
    env: Rc<lua::env::Env>,
}

fn ptr(v: &Value) -> Option<*const u8> {
    match v {
        Value::Table(t) => Some(Rc::as_ptr(&t.0) as *const u8),
        _ => None,
    }
}

impl Bridge {
    pub fn new(env: Rc<lua::env::Env>) -> Bridge {
        Bridge {
            stores: Stores::default(),
            lua: Vec::new(),
            ids: HashMap::new(),
            originals: Default::default(),
            lua_cfg: RefCell::new(Value::Nil),
            env,
        }
    }

    pub fn id(&self, store: &Value) -> Option<StoreId> {
        ptr(store).and_then(|p| self.ids.get(&p).copied())
    }

    /// Converts a Lua store and its parents; the store's id.
    pub fn add(&mut self, store: &Value) -> Result<StoreId, String> {
        if let Some(id) = self.id(store) {
            return Ok(id);
        }
        let conv = Converter {
            originals: &self.originals,
        };
        let mods = get(store, "mods");
        let mut typed = if let Value::Table(by_name) = &mods {
            let mut db = Store::db();
            for (k, list) in by_name.entries() {
                let name = match &k {
                    Key::Str(s) => Sym::new(&s.to_str_lossy()),
                    k => return Err(format!("mod name {k:?}")),
                };
                let mut bucket = Vec::new();
                for m in array(&list) {
                    let m = conv.mod_(&m).map_err(|e| format!("{name}: {e}"))?;
                    bucket.push(m);
                }
                if let Mods::Db(map) = &mut db.mods {
                    map.insert(name, bucket);
                }
            }
            db
        } else {
            let mut list = Store::list();
            for m in array(store) {
                list.add(conv.mod_(&m)?);
            }
            list
        };
        for (field, is_cond) in [("conditions", true), ("multipliers", false)] {
            if let Value::Table(t) = get(store, field) {
                for (k, x) in t.entries() {
                    let Key::Str(name) = k else { continue };
                    let name = Sym::new(&name.to_str_lossy());
                    if is_cond {
                        typed.conditions.insert(name, x.truthy());
                    } else if let Some(n) = lua::to_number(&x) {
                        typed.multipliers.insert(name, n);
                    }
                }
            }
        }
        let id = self.stores.push(typed);
        self.lua.push(store.clone());
        self.ids.insert(ptr(store).unwrap(), id);
        let parent = get(store, "parent");
        if parent.truthy() {
            let p = self.add(&parent)?;
            self.stores.stores[id].parent = Some(p);
        }
        Ok(id)
    }

    fn calc_lib(&self, name: &str, args: &[Value]) -> Value {
        let f = get(&self.env.global("calcLib"), name);
        first(rt::call(&f, args).unwrap_or_default())
    }

    /// `getActor(store, actorType)` of `ModStore.lua`
    fn get_actor(&self, store: &Value, actor_type: &str) -> Value {
        let actor = get(store, "actor");
        if actor_type == "player" {
            let p = get(&actor, "player");
            if p.truthy() {
                return p;
            }
            let parent = get(&actor, "parent");
            let p = if parent.truthy() {
                get(&parent, "player")
            } else {
                parent
            };
            if p.truthy() {
                return p;
            }
            let enemy = get(&actor, "enemy");
            if enemy.truthy() {
                get(&enemy, "player")
            } else {
                enemy
            }
        } else {
            get(&actor, actor_type)
        }
    }
}

impl Actors for Bridge {
    fn actor_store(&self, from: StoreId, actor: Sym) -> Option<StoreId> {
        let a = self.get_actor(&self.lua[from], actor.as_str());
        if !a.truthy() {
            return None;
        }
        let db = get(&a, "modDB");
        Some(
            self.id(&db)
                .unwrap_or_else(|| panic!("the modDB of actor '{actor}' was not converted")),
        )
    }

    fn output(&self, store: StoreId, stat: Sym) -> Option<f64> {
        let out = get(&get(&self.lua[store], "actor"), "output");
        if !out.truthy() {
            return None;
        }
        let v = get(&out, stat.as_str());
        if v.truthy() {
            Some(lua::to_number(&v).unwrap_or_else(|| panic!("output {stat} is {v:?}")))
        } else {
            None
        }
    }

    fn special_stat(&self, store: StoreId, stat: Sym, _cfg: Option<&Cfg>) -> Option<f64> {
        if !matches!(
            stat.as_str(),
            "ManaReservedPercent" | "LifeReservedPercent" | "ManaUnreserved"
        ) {
            return None;
        }
        let cfg = self.lua_cfg.borrow().clone();
        let r = rt::method(
            &self.lua[store],
            &LuaStr::from("GetStat"),
            &[s(stat.as_str()), cfg],
        )
        .unwrap_or_else(|e| panic!("GetStat {stat}: {e}"));
        Some(lua::to_number(&first(r)).unwrap_or(0.0))
    }

    fn one_handed_added(&self, store: StoreId, var: Sym) -> Option<bool> {
        let actor = get(&self.lua[store], "actor");
        let pick = |w: &str| {
            let d = get(&actor, w);
            (d.truthy() && get(&d, "countsAsAll1H").truthy()).then_some(d)
        };
        let all = pick("weaponData1").or_else(|| pick("weaponData2"))?;
        let v = get(&all, &format!("Added{var}"));
        (!v.is_nil()).then(|| v.truthy())
    }

    fn monster_tags(&self, store: StoreId) -> Option<Vec<Rc<str>>> {
        let actor = get(&self.lua[store], "actor");
        let md = get(&actor, "minionData");
        let tags = if md.truthy() {
            get(&md, "monsterTags")
        } else {
            Value::Nil
        };
        let Value::Table(t) = tags else {
            return None;
        };
        Some(
            t.entries()
                .into_iter()
                .map(|(_, v)| text(&v).unwrap())
                .collect(),
        )
    }

    fn item_condition(&self, store: StoreId, tag: &ItemCondition, _cfg: Option<&Cfg>) -> bool {
        // Evaluated by the translated EvalMod on a mod with this tag alone
        let lua_tag = self.originals.borrow().item_conditions[&(tag as *const _)].clone();
        let m = TableRef::new();
        m.set_str("value", Value::Number(1.0));
        m.push(lua_tag);
        let cfg = self.lua_cfg.borrow().clone();
        let r = rt::method(
            &self.lua[store],
            &LuaStr::from("EvalMod"),
            &[Value::Table(m), cfg],
        )
        .unwrap_or_else(|e| panic!("EvalMod ItemCondition: {e}"));
        first(r).truthy()
    }

    fn gem_is_type(&self, _cfg: &Cfg, keyword: &str) -> bool {
        let lua_cfg = self.lua_cfg.borrow().clone();
        let gem = get(&lua_cfg, "skillGem");
        gem.truthy() && self.calc_lib("gemIsType", &[gem, s(keyword)]).truthy()
    }

    fn game_id(&self, gem_name: &str) -> Option<Rc<str>> {
        match self.calc_lib("getGameIdFromGemName", &[s(gem_name), Value::Bool(true)]) {
            Value::Str(x) => Some(x.to_str_lossy().into()),
            _ => None,
        }
    }

    fn high_precision(&self, name: Sym, typ: ModType) -> Option<f64> {
        let hp = get(
            &get(&self.env.global("data"), "highPrecisionMods"),
            name.as_str(),
        );
        if !hp.truthy() {
            return None;
        }
        let p = get(&hp, typ.name());
        p.truthy().then(|| lua::to_number(&p).unwrap())
    }
}

/// Calls a translated store method, `Err` with its message on a Lua error.
pub fn call(store: &Value, method: &str, args: &[Value]) -> LResult<Vec<Value>> {
    rt::method(store, &LuaStr::from(method), args)
}
