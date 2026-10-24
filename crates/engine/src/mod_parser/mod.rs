//! Port of upstream `src/Modules/ModParser.lua` (v2.67.2).
//!
//! The pattern tables are imported from upstream's source by
//! `importer::mod_parser`. Their closures are ported to Rust: most are
//! translated by `cargo run -p importer --bin portgen` into `generated.rs`,
//! the rest by hand in `ported.rs`. Each port records the hash of the Lua
//! source it was made from, and [`ModParser::new`] fails if upstream's source
//! changed, so a changed closure cannot go unnoticed.

mod generated;
mod ported;

use crate::data::Data;
use crate::mod_tools::create_mod_args;
use crate::rt::*;
use lua::{Closure, LuaStr, TableRef, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A Rust port of one upstream closure.
pub struct Port {
    /// Where the closure is stored, as the importer names it
    pub key: &'static str,
    /// FNV-1a of the closure's Lua source
    pub hash: u64,
    pub f: fn(&Ctx, &[Value]) -> Vec<Value>,
}

type NativeFn = Rc<dyn Fn(&Ctx, &[Value]) -> Vec<Value>>;
type ValueFn = Rc<dyn Fn(&Ctx, &[Value]) -> Value>;

/// A value in a pattern table.
#[derive(Clone)]
enum Entry {
    Value(Value),
    Func(NativeFn),
}

impl Entry {
    fn is_func(&self) -> bool {
        matches!(self, Entry::Func(_))
    }
}

/// A pattern table in source order.
#[derive(Clone, Default)]
struct List {
    entries: Vec<(LuaStr, Entry)>,
    index: HashMap<LuaStr, usize>,
    /// Which entries a scan has selected, for rule coverage
    hits: RefCell<Vec<bool>>,
}

impl List {
    fn set(&mut self, k: LuaStr, v: Entry) {
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, v));
            }
        }
    }

    fn get(&self, k: &[u8]) -> Option<&Entry> {
        self.index.get(&LuaStr::new(k)).map(|&i| &self.entries[i].1)
    }
}

/// What closures can reach of their enclosing module.
pub struct Ctx {
    pub data: Data,
    values: HashMap<&'static str, Value>,
}

impl Ctx {
    /// A module-level value: `ModFlag`, `data`, `gemIdLookup`, ...
    pub fn value(&self, name: &str) -> Value {
        match self.values.get(name) {
            Some(v) => v.clone(),
            None => panic!("ModParser value {name} is not available"),
        }
    }

    fn gem_id_lookup(&self) -> Value {
        self.value("gemIdLookup")
    }

    /// `mod(...)`, i.e. `modLib.createMod`
    pub fn mod_(&self, args: Vec<Value>) -> Vec<Value> {
        vec![Value::Table(create_mod_args(&args))]
    }

    /// `flag(name, ...)`
    pub fn flag(&self, args: Vec<Value>) -> Vec<Value> {
        let mut a = vec![arg(&args, 0), s("FLAG"), Value::Bool(true)];
        a.extend(args.into_iter().skip(1));
        self.mod_(a)
    }

    /// `firstToUpper(str)`
    pub fn first_to_upper(&self, args: Vec<Value>) -> Vec<Value> {
        vec![first(gsub_fn(
            &arg(&args, 0),
            &s("^%l"),
            &mut upper_fn,
            &nil(),
        ))]
    }

    /// `dealNoNonDamageType(dmgType, forMinion)`
    pub fn deal_no_non_damage_type(&self, args: Vec<Value>) -> Vec<Value> {
        let dmg_type = first(self.first_to_upper(vec![arg(&args, 0)]));
        let for_minion = arg(&args, 1);
        let list = self.value("damageTypeList");
        if !is_value_in_array(&list, &dmg_type) {
            return vec![];
        }
        let mods = tbl(vec![], vec![]);
        for (_, damage_type) in ipairs(&list) {
            if !damage_type.raw_eq(&dmg_type) {
                let deal_no = first(self.flag(vec![concat(&s("DealNo"), &damage_type)]));
                let m = if for_minion.truthy() {
                    first(self.mod_(vec![
                        s("MinionModifier"),
                        s("LIST"),
                        tbl(vec![(s("mod"), deal_no)], vec![]),
                    ]))
                } else {
                    deal_no
                };
                t_insert(&mods, m);
            }
        }
        vec![mods]
    }

    /// `grantedExtraSkill(name, level, noSupports)`
    pub fn granted_extra_skill(&self, args: Vec<Value>) -> Vec<Value> {
        let name = first(gsub(&arg(&args, 0), &s(" skill"), &s(""), &nil()));
        let lookup = self.gem_id_lookup();
        let id = index(&lookup, &name);
        if id.truthy() {
            let v = tbl(
                vec![
                    (s("skillId"), id),
                    (s("level"), tonumber(&arg(&args, 1))),
                    (s("noSupports"), arg(&args, 2)),
                ],
                vec![],
            );
            return vec![tbl(
                vec![],
                vec![first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v]))],
            )];
        }
        vec![]
    }

    /// `triggerExtraSkill(name, level, options)`
    pub fn trigger_extra_skill(&self, args: Vec<Value>) -> Vec<Value> {
        let options = arg(&args, 2);
        let options = if options.truthy() {
            options
        } else {
            tbl(vec![], vec![])
        };
        let name = first(gsub(&arg(&args, 0), &s(" skill"), &s(""), &nil()));
        if field(&options, "sourceSkill").truthy() {
            set_index(
                &options,
                s("sourceSkill"),
                first(gsub(
                    &field(&options, "sourceSkill"),
                    &s(" skill"),
                    &s(""),
                    &nil(),
                )),
            );
        }
        let mods = tbl(vec![], vec![]);
        let lookup = self.gem_id_lookup();
        let id = index(&lookup, &name);
        if id.truthy() {
            let v = tbl(
                vec![
                    (s("skillId"), id.clone()),
                    (s("level"), tonumber(&arg(&args, 1))),
                    (s("noSupports"), field(&options, "noSupports")),
                    (s("triggered"), Value::Bool(true)),
                    (s("source"), field(&options, "sourceSkill")),
                    (
                        s("triggerChance"),
                        tonumber(&field(&options, "triggerChance")),
                    ),
                ],
                vec![],
            );
            t_insert(&mods, first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v])));
        }
        let skill_tag = || {
            tbl(
                vec![(s("type"), s("SkillId")), (s("skillId"), id.clone())],
                vec![],
            )
        };
        if field(&options, "ignoreHexproof").truthy() {
            let v = tbl(
                vec![
                    (s("key"), s("ignoreHexproof")),
                    (s("value"), Value::Bool(true)),
                ],
                vec![],
            );
            t_insert(
                &mods,
                first(self.mod_(vec![s("SkillData"), s("LIST"), v, skill_tag()])),
            );
        }
        if field(&options, "onCrit").truthy() {
            let inner = tbl(
                vec![
                    (s("key"), s("triggerOnCrit")),
                    (s("value"), Value::Bool(true)),
                ],
                vec![],
            );
            let inner = first(self.mod_(vec![s("SkillData"), s("LIST"), inner]));
            let v = tbl(vec![(s("mod"), inner)], vec![]);
            t_insert(
                &mods,
                first(self.mod_(vec![s("ExtraSkillMod"), s("LIST"), v, skill_tag()])),
            );
        }
        vec![mods]
    }

    /// `extraSupport(name, level, slot)`
    pub fn extra_support(&self, args: Vec<Value>) -> Vec<Value> {
        let name = arg(&args, 0);
        let lookup = self.gem_id_lookup();
        let mut skill_id = index(&lookup, &name);
        if !skill_id.truthy() {
            skill_id = index(
                &lookup,
                &first(gsub(&name, &s("^increased "), &s(""), &nil())),
            );
        }
        if !skill_id.truthy() {
            skill_id = index(
                &lookup,
                &first(gsub(&name, &s(" support$"), &s(""), &nil())),
            );
        }
        let mut slot = arg(&args, 2);
        if slot.raw_eq(&s("main hand")) || slot.raw_eq(&s("main hand weapon")) {
            slot = s("Weapon 1");
        } else if slot.raw_eq(&s("off hand")) || slot.raw_eq(&s("off hand weapon")) {
            slot = s("Weapon 2");
        } else if slot.truthy() {
            let spaced = concat(&s(" "), &slot);
            slot = str_sub(
                &first(gsub_fn(&spaced, &s("%W%l"), &mut upper_fn, &nil())),
                &n(2.0),
                &nil(),
            );
        } else {
            slot = s("{SlotName}");
        }
        let level = tonumber(&arg(&args, 1));
        if !skill_id.truthy() {
            return vec![];
        }
        let data = self.value("data");
        let socketed = || {
            tbl(
                vec![(s("type"), s("SocketedIn")), (s("slotName"), slot.clone())],
                vec![],
            )
        };
        let skill_name = field(&index(&field(&data, "skills"), &skill_id), "name");
        let gem_id = index(
            &field(&data, "gemForBaseName"),
            &lower(&concat(&skill_name, &s(" Support"))),
        );
        if gem_id.truthy() {
            let gem = index(&field(&data, "gems"), &gem_id);
            let v = tbl(
                vec![
                    (s("skillId"), field(&gem, "grantedEffectId")),
                    (s("level"), level.clone()),
                ],
                vec![],
            );
            let mods = tbl(
                vec![],
                vec![first(self.mod_(vec![
                    s("ExtraSupport"),
                    s("LIST"),
                    v,
                    socketed(),
                ]))],
            );
            let secondary = field(&gem, "secondaryGrantedEffect");
            if secondary.truthy() {
                let v = tbl(
                    vec![
                        (s("skillId"), field(&gem, "secondaryGrantedEffectId")),
                        (s("level"), level.clone()),
                    ],
                    vec![],
                );
                if field(&secondary, "support").truthy() {
                    t_insert(
                        &mods,
                        first(self.mod_(vec![s("ExtraSupport"), s("LIST"), v, socketed()])),
                    );
                } else {
                    t_insert(&mods, first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v])));
                }
            }
            vec![mods]
        } else {
            let v = tbl(vec![(s("skillId"), skill_id), (s("level"), level)], vec![]);
            vec![tbl(
                vec![],
                vec![first(self.mod_(vec![
                    s("ExtraSupport"),
                    s("LIST"),
                    v,
                    socketed(),
                ]))],
            )]
        }
    }

    /// `explodeFunc(chance, amount, type, ...)`
    pub fn explode_func(&self, args: Vec<Value>) -> Vec<Value> {
        let (chance, amount, typ) = (arg(&args, 0), arg(&args, 1), arg(&args, 2));
        let mut amount_number = tonumber(&amount);
        if !amount_number.truthy() {
            amount_number = if amount.raw_eq(&s("tenth")) {
                n(10.0)
            } else if amount.raw_eq(&s("quarter")) {
                n(25.0)
            } else {
                Value::Bool(false)
            };
        }
        if !amount_number.truthy() {
            return vec![];
        }
        let v = tbl(
            vec![
                (s("type"), first(self.first_to_upper(vec![typ]))),
                (s("chance"), div(&chance, &n(100.0))),
                (s("amount"), amount_number),
                (s("keyOfScaledMod"), s("chance")),
            ],
            vec![],
        );
        let mut mod_args = vec![s("ExplodeMod"), s("LIST"), v];
        mod_args.extend(args.into_iter().skip(3));
        vec![tbl(
            vec![],
            vec![
                first(self.mod_(mod_args)),
                first(self.flag(vec![s("CanExplode")])),
            ],
        )]
    }

    /// `getSimpleConv(...)`: the jewel function is a closure run by the calcs.
    pub fn get_simple_conv(&self, args: Vec<Value>) -> Vec<Value> {
        vec![closure("getSimpleConv", args)]
    }

    /// `getPerStat(...)`, see [`Ctx::get_simple_conv`].
    pub fn get_per_stat(&self, args: Vec<Value>) -> Vec<Value> {
        vec![closure("getPerStat", args)]
    }

    /// `getThreshold(...)`, see [`Ctx::get_simple_conv`].
    pub fn get_threshold(&self, args: Vec<Value>) -> Vec<Value> {
        vec![closure("getThreshold", args)]
    }
}

fn closure(name: &str, upvalues: Vec<Value>) -> Value {
    Value::Closure(Rc::new(Closure {
        name: name.into(),
        upvalues,
    }))
}

/// Error from [`ModParser::new`].
#[derive(Debug)]
pub enum InitError {
    /// An upstream closure has no Rust port
    Unported { key: String },
    /// An upstream closure changed since it was ported
    Changed {
        key: String,
        expected: u64,
        found: u64,
    },
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Unported { key } => write!(f, "ModParser closure {key} has no Rust port"),
            InitError::Changed {
                key,
                expected,
                found,
            } => write!(
                f,
                "ModParser closure {key} changed upstream (ported from {expected:016x}, now {found:016x}); \
                 rerun portgen or update ported.rs"
            ),
        }
    }
}

impl std::error::Error for InitError {}

pub struct ModParser {
    ctx: Ctx,
    form_list: List,
    mod_name_list: List,
    mod_flag_list: List,
    pre_flag_list: List,
    mod_tag_list: List,
    special_mod_list: List,
    unsupported_mod_list: List,
    suffix_types: List,
    dmg_types: TableRef,
    pen_types: List,
    flag_types: List,
    regen_types: TableRef,
    degen_types: TableRef,
    cost_types: List,
    base_cost_types: List,
    skill_name_list: List,
    pre_skill_name_list: List,
    jewel_func_list: List,
    cluster_jewel_skills: List,
    cache: RefCell<HashMap<Vec<u8>, (Value, Value)>>,
}

/// Mod list and unparsed rest, as `modLib.parseMod` returns them.
pub type Parsed = (Value, Value);

fn list_from(t: &Value, ports: &HashMap<&str, &Port>) -> Result<List, InitError> {
    let mut list = List::default();
    for (k, v) in t.as_table().expect("pattern table").entries() {
        let key = match k.to_value() {
            Value::Str(s) => s,
            other => panic!("pattern table key {other:?}"),
        };
        let entry = match v {
            Value::Func(f) => match ports.get(&*f.key) {
                Some(p) if p.hash == f.source_hash => {
                    let pf = p.f;
                    Entry::Func(Rc::new(move |c: &Ctx, a: &[Value]| pf(c, a)))
                }
                Some(p) => {
                    return Err(InitError::Changed {
                        key: f.key.to_string(),
                        expected: p.hash,
                        found: f.source_hash,
                    });
                }
                None => {
                    return Err(InitError::Unported {
                        key: f.key.to_string(),
                    });
                }
            },
            v => Entry::Value(v),
        };
        list.set(key, entry);
    }
    Ok(list)
}

impl ModParser {
    /// Builds the parser from the imported tables, running the setup code
    /// of ModParser.lua.
    pub fn new(data: Data, tables: &TableRef) -> Result<ModParser, InitError> {
        let mut ports: HashMap<&str, &Port> = HashMap::new();
        for p in generated::PORTS.iter().chain(ported::PORTS) {
            ports.insert(p.key, p);
        }
        let t = |name: &str| tables.get_str(name);
        let mut values = HashMap::new();
        for g in ["ModFlag", "KeywordFlag", "SkillType"] {
            values.insert(g, data.global(g));
        }
        values.insert("data", Value::Table(data.data.clone()));
        values.insert("damageTypeList", t("damageTypeList"));
        values.insert("conquerorList", t("conquerorList"));
        let gem_id_lookup = t("gemIdLookup");
        values.insert("gemIdLookup", gem_id_lookup.clone());
        let ctx = Ctx { data, values };

        // gemIdLookup: every non-hidden skill, plus hidden ones granted by items or the tree
        let skills = ctx.data.get("skills");
        let mut skill_ids: Vec<Value> = pairs(&skills).into_iter().map(|(k, _)| k).collect();
        skill_ids.sort_by(|a, b| {
            a.as_str()
                .unwrap()
                .as_bytes()
                .cmp(b.as_str().unwrap().as_bytes())
        });
        for id in &skill_ids {
            let ge = index(&skills, id);
            if !field(&ge, "hidden").truthy()
                || field(&ge, "fromItem").truthy()
                || field(&ge, "fromTree").truthy()
            {
                set_index(&gem_id_lookup, lower(&field(&ge, "name")), field(&ge, "id"));
            }
        }

        let mut special = list_from(&t("specialModList"), &ports)?;
        for (_, name) in ipairs(&ctx.data.get("keystones")) {
            let have = first(ctx.first_to_upper(vec![name.clone()]));
            let have = first(gsub_fn(&have, &s(" %l"), &mut upper_fn, &nil()));
            let have = first(gsub(&have, &s(" "), &s(""), &nil()));
            let mods = tbl(
                vec![],
                vec![
                    first(ctx.mod_(vec![s("Keystone"), s("LIST"), name.clone()])),
                    first(ctx.flag(vec![concat(&s("Condition:Have"), &have)])),
                ],
            );
            special.set(lua_str(&lower(&name)), Entry::Value(mods));
        }
        let cluster_jewels = ctx.data.get("clusterJewels");
        for (_, name) in ipairs(&field(&cluster_jewels, "keystones")) {
            let mods = tbl(
                vec![],
                vec![first(ctx.mod_(vec![
                    s("Keystone"),
                    s("LIST"),
                    name.clone(),
                ]))],
            );
            special.set(lua_str(&lower(&name)), Entry::Value(mods));
        }
        let mut anchored = List::default();
        for (k, v) in special.entries {
            let mut key = b"^".to_vec();
            key.extend_from_slice(k.as_bytes());
            key.push(b'$');
            anchored.set(key.into(), v);
        }
        let mut special = anchored;

        let resource_types = t("resourceTypes");
        let maximum = TableRef::new();
        for (resource, v) in pairs(&resource_types) {
            maximum.set(concat(&s("maximum "), &resource), v);
        }
        for (resource, v) in maximum.entries() {
            set_index(&resource_types, resource.to_value(), v);
        }
        let append_mod = |suffix: &str| {
            let out = TableRef::new();
            for (sub_line, mods) in pairs(&resource_types) {
                if let Value::Str(_) = mods {
                    out.set(sub_line, concat(&mods, &s(suffix)));
                } else {
                    let l = TableRef::new();
                    for (_, m) in ipairs(&mods) {
                        l.push(concat(&m, &s(suffix)));
                    }
                    out.set(sub_line, Value::Table(l));
                }
            }
            out
        };
        let regen_types = append_mod("Regen");
        let degen_types = append_mod("Degen");
        let cost_types = list_from(&Value::Table(append_mod("Cost")), &ports)?;
        let base_cost_types = list_from(&Value::Table(append_mod("CostNoMult")), &ports)?;

        let mut skill_name_list = list_from(&t("skillNameList"), &ports)?;
        let mut pre_skill_name_list = List::default();
        ctx.add_gem_patterns(&mut skill_name_list, &mut pre_skill_name_list, &mut special);

        let jewel_func_list = ctx.jewel_func_list(
            &t("jewelOtherFuncs"),
            &t("jewelSelfFuncs"),
            &t("jewelSelfUnallocFuncs"),
            &t("jewelThresholdFuncs"),
        );
        let cluster_jewel_skills = ctx.cluster_jewel_skills(&cluster_jewels);

        Ok(ModParser {
            form_list: list_from(&t("formList"), &ports)?,
            mod_name_list: list_from(&t("modNameList"), &ports)?,
            mod_flag_list: list_from(&t("modFlagList"), &ports)?,
            pre_flag_list: list_from(&t("preFlagList"), &ports)?,
            mod_tag_list: list_from(&t("modTagList"), &ports)?,
            special_mod_list: special,
            unsupported_mod_list: list_from(&t("unsupportedModList"), &ports)?,
            suffix_types: list_from(&t("suffixTypes"), &ports)?,
            dmg_types: t("dmgTypes").as_table().expect("dmgTypes").clone(),
            pen_types: list_from(&t("penTypes"), &ports)?,
            flag_types: list_from(&t("flagTypes"), &ports)?,
            regen_types,
            degen_types,
            cost_types,
            base_cost_types,
            skill_name_list,
            pre_skill_name_list,
            jewel_func_list,
            cluster_jewel_skills,
            ctx,
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// Patterns no scan has selected so far, by table: rule coverage of the
    /// lines parsed with this parser.
    pub fn unhit_rules(&self) -> Vec<(&'static str, String)> {
        let lists: [(&'static str, &List); 10] = [
            ("formList", &self.form_list),
            ("modNameList", &self.mod_name_list),
            ("modFlagList", &self.mod_flag_list),
            ("preFlagList", &self.pre_flag_list),
            ("modTagList", &self.mod_tag_list),
            ("specialModList", &self.special_mod_list),
            ("suffixTypes", &self.suffix_types),
            ("flagTypes", &self.flag_types),
            ("skillNameList", &self.skill_name_list),
            ("preSkillNameList", &self.pre_skill_name_list),
        ];
        let mut out = Vec::new();
        for (name, list) in lists {
            let hits = list.hits.borrow();
            for (i, (pattern, _)) in list.entries.iter().enumerate() {
                if !hits.get(i).copied().unwrap_or(false) {
                    out.push((name, pattern.to_string()));
                }
            }
        }
        out
    }

    /// Preloads the memo cache with upstream's `Data/ModCache.lua` (line ->
    /// `{ modList, extra }`), as upstream does on startup. Cached results are
    /// what desktop PoB uses for every line it ships.
    pub fn preload(&self, mod_cache: &TableRef) {
        let mut cache = self.cache.borrow_mut();
        for (line, entry) in mod_cache.entries() {
            let line = line.to_value();
            let line = line.as_str().expect("ModCache key").as_bytes().to_vec();
            cache.insert(line, (index(&entry, &n(1.0)), index(&entry, &n(2.0))));
        }
    }

    /// `modLib.parseMod(line, isComb)`: the mod list (or nil) and the part
    /// of the line that was not understood (or nil). Results are cached and
    /// returned as copies, as upstream does.
    pub fn parse(&self, line: &[u8]) -> Parsed {
        if let Some((m, e)) = self.cache.borrow().get(line) {
            return (copy_value(m), copy_value(e));
        }
        let line_v = sb(line);
        let (mut mod_list, mut extra) = self.parse_mod(&line_v, 1);
        if mod_list.truthy() && extra.truthy() {
            (mod_list, extra) = self.parse_mod(&line_v, 2);
        }
        let result = (copy_value(&mod_list), copy_value(&extra));
        self.cache
            .borrow_mut()
            .insert(line.to_vec(), (mod_list, extra));
        result
    }

    /// `scan(line, patternList, plain)`
    fn scan<'a>(
        &self,
        line: &Value,
        list: &'a List,
        plain: bool,
    ) -> (Option<&'a Entry>, Value, Vec<Value>) {
        let lower_line = lower(line);
        let lower_bytes = lower_line.as_str().expect("string").as_bytes().to_vec();
        let mut best: Option<(usize, usize, usize, &Entry, Vec<Value>)> = None;
        let mut best_i = 0;
        for (i, (pattern, val)) in list.entries.iter().enumerate() {
            let found = lua::pattern::lua_find(&lower_bytes, pattern.as_bytes(), 1, plain)
                .unwrap_or_else(|e| panic!("bad pattern {pattern:?}: {e:?}"));
            if let Some((index, end, caps)) = found {
                let better = match &best {
                    None => true,
                    Some((bi, be, bl, _, _)) => {
                        index < *bi
                            || (index == *bi && (end > *be || (end == *be && pattern.len() > *bl)))
                    }
                };
                if better {
                    let mut caps = caps;
                    caps.truncate(5);
                    best = Some((index, end, pattern.len(), val, caps));
                    best_i = i;
                }
            }
        }
        if best.is_some() {
            let mut hits = list.hits.borrow_mut();
            if hits.len() < list.entries.len() {
                hits.resize(list.entries.len(), false);
            }
            hits[best_i] = true;
        }
        match best {
            Some((start, end, _, val, caps)) => {
                let bytes = line.as_str().expect("string").as_bytes();
                let mut rest = bytes[..start - 1].to_vec();
                rest.extend_from_slice(&bytes[end.min(bytes.len())..]);
                (Some(val), sb(&rest), caps)
            }
            None => (None, line.clone(), vec![]),
        }
    }

    fn call(&self, entry: &Entry, args: &[Value]) -> Value {
        match entry {
            Entry::Func(f) => first(f(&self.ctx, args)),
            Entry::Value(v) => v.clone(),
        }
    }

    fn entry_value(entry: Option<&Entry>) -> Value {
        match entry {
            Some(Entry::Value(v)) => v.clone(),
            Some(Entry::Func(_)) => panic!("unexpected function entry"),
            None => Value::Nil,
        }
    }

    /// `parseMod(line, order)`
    fn parse_mod(&self, line_in: &Value, order: u8) -> Parsed {
        let ctx = &self.ctx;
        let line_lower = lower(line_in);
        let lower_bytes = line_lower.as_str().expect("string").as_bytes().to_vec();

        // Jewel functions with captures
        for (pattern, entry) in &self.jewel_func_list.entries {
            let found = lua::pattern::lua_find(&lower_bytes, pattern.as_bytes(), 1, false)
                .expect("jewel pattern");
            if let Some((_, _, caps)) = found
                && caps.first().is_some_and(Value::truthy)
            {
                let Entry::Value(pv) = entry else {
                    unreachable!()
                };
                let func = jewel_func_call(pv, pattern, &caps);
                let v = tbl(
                    vec![(s("func"), func), (s("type"), field(pv, "type"))],
                    vec![],
                );
                return (
                    tbl(
                        vec![],
                        vec![first(ctx.mod_(vec![s("JewelFunc"), s("LIST"), v]))],
                    ),
                    nil(),
                );
            }
        }
        if let Some(Entry::Value(jewel_func)) = self.jewel_func_list.get(&lower_bytes) {
            return (
                tbl(
                    vec![],
                    vec![first(ctx.mod_(vec![
                        s("JewelFunc"),
                        s("LIST"),
                        jewel_func.clone(),
                    ]))],
                ),
                nil(),
            );
        }
        if let Some(Entry::Value(skill)) = self.cluster_jewel_skills.get(&lower_bytes) {
            return (skill.clone(), nil());
        }
        if self.unsupported_mod_list.get(&lower_bytes).is_some() {
            return (tbl(vec![], vec![]), line_in.clone());
        }
        let (special_mod, special_line, cap) = self.scan(line_in, &self.special_mod_list, false);
        if let Some(special_mod) = special_mod
            && len(&special_line).as_number() == Some(0.0)
        {
            return match special_mod {
                Entry::Func(f) => {
                    let mut args = vec![tonumber(&nth(&cap, 0))];
                    args.extend(cap.iter().cloned());
                    let r = f(ctx, &args);
                    (nth(&r, 0), nth(&r, 1))
                }
                Entry::Value(v) => (copy_table(v), nil()),
            };
        }

        // Add-to-cluster-jewel special
        let add_to_cluster = first(str_match(
            line_in,
            &s("^Added Small Passive Skills also grant: (.+)$"),
            &nil(),
        ));
        if add_to_cluster.truthy() {
            return (
                tbl(
                    vec![],
                    vec![first(ctx.mod_(vec![
                        s("AddToClusterJewelNode"),
                        s("LIST"),
                        add_to_cluster,
                    ]))],
                ),
                nil(),
            );
        }

        let mut line = concat(line_in, &s(" "));

        // Flag/tag specification at the start of the line
        let (pre_flag_entry, rest, pre_flag_cap) = self.scan(&line, &self.pre_flag_list, false);
        line = rest;
        let pre_flag = match pre_flag_entry {
            Some(e) if e.is_func() => self.call(e, &pre_flag_cap),
            e => Self::entry_value(e),
        };

        // Skill name at the start of the line
        let (skill_tag_entry, rest, _) = self.scan(&line, &self.pre_skill_name_list, false);
        line = rest;
        let mut skill_tag = Self::entry_value(skill_tag_entry);

        // Modifier form
        let (mod_form_entry, rest, mut form_cap) = self.scan(&line, &self.form_list, false);
        line = rest;
        let Some(mod_form_entry) = mod_form_entry else {
            return (nil(), line);
        };
        let mod_form = Self::entry_value(Some(mod_form_entry));
        let mod_form = mod_form.as_str().expect("form").to_string();

        // Tags (per-charge, conditionals)
        let scan_tag = |line: &Value| -> (Value, Value) {
            let (entry, rest, tag_cap) = self.scan(line, &self.mod_tag_list, false);
            let tag = match entry {
                Some(e) if e.is_func() => {
                    let c1 = nth(&tag_cap, 0);
                    let first_arg = if first(str_match(&c1, &s("%d+"), &nil())).truthy() {
                        tonumber(&c1)
                    } else {
                        c1
                    };
                    let mut args = vec![first_arg];
                    args.extend(tag_cap.iter().cloned());
                    self.call(e, &args)
                }
                e => Self::entry_value(e),
            };
            (tag, rest)
        };
        let (mod_tag, rest) = scan_tag(&line);
        line = rest;
        let mut mod_tag2 = nil();
        if mod_tag.truthy() {
            let (t2, rest) = scan_tag(&line);
            mod_tag2 = t2;
            line = rest;
        }

        // Modifier name and skill name
        let mut mod_name;
        if order == 2 && !skill_tag.truthy() {
            let (e, rest, _) = self.scan(&line, &self.skill_name_list, false);
            skill_tag = Self::entry_value(e);
            line = rest;
        }
        let scan_rest_name =
            |line: Value| -> Value { self.scan(&line, &self.mod_name_list, true).1 };
        match mod_form.as_str() {
            "PEN" | "BASECOST" | "TOTALCOST" => {
                let list = match mod_form.as_str() {
                    "PEN" => &self.pen_types,
                    "BASECOST" => &self.base_cost_types,
                    _ => &self.cost_types,
                };
                let (e, rest, _) = self.scan(&line, list, true);
                line = rest;
                mod_name = Self::entry_value(e);
                if !mod_name.truthy() {
                    return (tbl(vec![], vec![]), line);
                }
                line = scan_rest_name(line);
            }
            "FLAG" => {
                let (e, rest, _) = self.scan(&line, &self.flag_types, false);
                line = rest;
                let v = Self::entry_value(e);
                if form_cap.is_empty() {
                    form_cap.push(nil());
                }
                form_cap[0] = v.clone();
                if !v.truthy() {
                    return (nil(), line);
                }
                let (e, rest, _) = self.scan(&line, &self.mod_name_list, true);
                line = rest;
                mod_name = Self::entry_value(e);
            }
            _ => {
                let (e, rest, _) = self.scan(&line, &self.mod_name_list, true);
                line = rest;
                mod_name = Self::entry_value(e);
            }
        }
        if order == 1 && !skill_tag.truthy() {
            let (e, rest, _) = self.scan(&line, &self.skill_name_list, false);
            skill_tag = Self::entry_value(e);
            line = rest;
        }

        // Flags
        let (e, rest, _) = self.scan(&line, &self.mod_flag_list, true);
        let mut mod_flag = Self::entry_value(e);
        line = rest;

        // Value and type according to form
        let cap0 = nth(&form_cap, 0);
        let mut mod_value = {
            let num = tonumber(&cap0);
            if num.truthy() { num } else { cap0.clone() }
        };
        let mut mod_type = s("BASE");
        let mut mod_suffix = nil();
        let mut mod_extra_tags = nil();
        let scan_suffix = |line: &Value| -> (Value, Value) {
            let (e, rest, _) = self.scan(line, &self.suffix_types, true);
            (Self::entry_value(e), rest)
        };
        let hand_attack_tag = || {
            tbl(
                vec![(
                    s("tag"),
                    tbl(
                        vec![(s("type"), s("Condition")), (s("var"), s("{Hand}Attack"))],
                        vec![],
                    ),
                )],
                vec![],
            )
        };
        let damage = |i: usize| index(&Value::Table(self.dmg_types.clone()), &nth(&form_cap, i));
        let min_max = |dt: &Value| tbl(vec![], vec![concat(dt, &s("Min")), concat(dt, &s("Max"))]);
        let min_max_value = || {
            tbl(
                vec![],
                vec![tonumber(&nth(&form_cap, 0)), tonumber(&nth(&form_cap, 1))],
            )
        };
        let keyword = |name: &str| field(&ctx.data.global("KeywordFlag"), name);
        match mod_form.as_str() {
            "INC" => mod_type = s("INC"),
            "RED" => {
                mod_value = neg(&mod_value);
                mod_type = s("INC");
            }
            "MORE" => mod_type = s("MORE"),
            "LESS" => {
                mod_value = neg(&mod_value);
                mod_type = s("MORE");
            }
            "BASE" | "GAIN" => (mod_suffix, line) = scan_suffix(&line),
            "LOSE" => {
                mod_value = neg(&mod_value);
                (mod_suffix, line) = scan_suffix(&line);
            }
            "GRANTS" => {
                mod_extra_tags = hand_attack_tag();
                (mod_suffix, line) = scan_suffix(&line);
            }
            "GRANTS_GLOBAL" => (mod_suffix, line) = scan_suffix(&line),
            "REMOVES" => {
                mod_value = neg(&mod_value);
                mod_extra_tags = hand_attack_tag();
                (mod_suffix, line) = scan_suffix(&line);
            }
            "CHANCE" => {}
            "REGENPERCENT" => {
                mod_name = self.regen_types.get(&nth(&form_cap, 1));
                mod_suffix = s("Percent");
            }
            "REGENFLAT" => mod_name = self.regen_types.get(&nth(&form_cap, 1)),
            "DEGENPERCENT" => {
                mod_name = self.degen_types.get(&nth(&form_cap, 1));
                mod_suffix = s("Percent");
            }
            "DEGENFLAT" => mod_name = self.degen_types.get(&nth(&form_cap, 1)),
            "DEGEN" => {
                let dt = damage(1);
                if !dt.truthy() {
                    return (tbl(vec![], vec![]), line);
                }
                mod_name = concat(&dt, &s("Degen"));
                mod_suffix = s("");
            }
            "DMG" | "DMGATTACKS" | "DMGSPELLS" | "DMGBOTH" => {
                let dt = damage(2);
                if !dt.truthy() {
                    return (tbl(vec![], vec![]), line);
                }
                mod_value = min_max_value();
                mod_name = min_max(&dt);
                if !mod_flag.truthy() {
                    match mod_form.as_str() {
                        "DMGATTACKS" => {
                            mod_flag = tbl(vec![(s("keywordFlags"), keyword("Attack"))], vec![])
                        }
                        "DMGSPELLS" => {
                            mod_flag = tbl(vec![(s("keywordFlags"), keyword("Spell"))], vec![])
                        }
                        "DMGBOTH" => {
                            mod_flag = tbl(
                                vec![(
                                    s("keywordFlags"),
                                    bor(&[keyword("Attack"), keyword("Spell")]),
                                )],
                                vec![],
                            )
                        }
                        _ => {}
                    }
                }
            }
            "FLAG" => {
                let is_table = matches!(mod_value, Value::Table(_));
                mod_name = if is_table {
                    field(&mod_value, "name")
                } else {
                    mod_value.clone()
                };
                mod_type = if is_table {
                    field(&mod_value, "type")
                } else {
                    s("FLAG")
                };
                mod_value = if is_table {
                    field(&mod_value, "value")
                } else {
                    Value::Bool(true)
                };
            }
            "OVERRIDE" => mod_type = s("OVERRIDE"),
            "DOUBLED" => {
                let mod_name_string;
                if let Value::Table(_) = mod_name {
                    mod_name_string = index(&mod_name, &n(1.0));
                    set_index(
                        &mod_name,
                        n(2.0),
                        concat(&concat(&s("Multiplier:"), &mod_name_string), &s("Doubled")),
                    );
                } else {
                    mod_name_string = mod_name.clone();
                    if mod_name.truthy() {
                        let second = concat(&concat(&s("Multiplier:"), &mod_name), &s("Doubled"));
                        mod_name = tbl(vec![], vec![mod_name.clone(), second]);
                    }
                }
                if mod_name.truthy() {
                    mod_type = tbl(vec![], vec![s("MORE"), s("OVERRIDE")]);
                    mod_value = tbl(vec![], vec![n(100.0), n(1.0)]);
                    mod_extra_tags = tbl(vec![(s("tag"), Value::Bool(true))], vec![]);
                    let tag = tbl(
                        vec![
                            (s("type"), s("Multiplier")),
                            (s("var"), concat(&mod_name_string, &s("Doubled"))),
                            (s("globalLimit"), n(100.0)),
                            (
                                s("globalLimitKey"),
                                concat(&mod_name_string, &s("DoubledLimit")),
                            ),
                        ],
                        vec![],
                    );
                    set_index(&mod_extra_tags, n(1.0), tbl(vec![(s("tag"), tag)], vec![]));
                }
            }
            _ => {}
        }

        if !mod_name.truthy() {
            return (tbl(vec![], vec![]), line);
        }

        // Combine flags and tags
        let mut flags = n(0.0);
        let mut keyword_flags = n(0.0);
        let tag_list = TableRef::new();
        let mut mod_tag_list: Option<TableRef> = None;
        let misc = TableRef::new();
        for d in [
            &mod_name,
            &pre_flag,
            &mod_flag,
            &mod_tag,
            &mod_tag2,
            &skill_tag,
            &mod_extra_tags,
        ] {
            if !matches!(d, Value::Table(_)) {
                continue;
            }
            let or_zero = |v: Value| if v.truthy() { v } else { n(0.0) };
            flags = bor(&[flags, or_zero(field(d, "flags"))]);
            keyword_flags = bor(&[keyword_flags, or_zero(field(d, "keywordFlags"))]);
            let first_entry = index(d, &n(1.0));
            if field(d, "tag").truthy() {
                if first_entry.truthy() && field(&first_entry, "tag").truthy() {
                    let l = TableRef::new();
                    for (i, entry) in ipairs(d) {
                        let sub = TableRef::new();
                        if field(&entry, "tag").truthy() {
                            sub.push(copy_table(&field(&entry, "tag")));
                        }
                        l.set(i, Value::Table(sub));
                    }
                    mod_tag_list = Some(l);
                } else {
                    tag_list.push(copy_table(&field(d, "tag")));
                }
            } else if field(d, "tagList").truthy() {
                if first_entry.truthy() && field(&first_entry, "tagList").truthy() {
                    let l = TableRef::new();
                    for (i, entry) in ipairs(d) {
                        let sub = TableRef::new();
                        for (_, tag) in ipairs(&field(&entry, "tagList")) {
                            sub.push(copy_table(&tag));
                        }
                        l.set(i, Value::Table(sub));
                    }
                    mod_tag_list = Some(l);
                } else {
                    for (_, tag) in ipairs(&field(d, "tagList")) {
                        tag_list.push(copy_table(&tag));
                    }
                }
            }
            for (k, v) in pairs(d) {
                misc.set(k, v);
            }
        }

        // Generate modifier list
        let names = if let Value::Table(_) = mod_name {
            mod_name.clone()
        } else {
            tbl(vec![], vec![mod_name.clone()])
        };
        let mod_list = TableRef::new();
        let suffix = if mod_suffix.truthy() {
            mod_suffix.clone()
        } else if misc.get_str("modSuffix").truthy() {
            misc.get_str("modSuffix")
        } else {
            s("")
        };
        for (i, name) in ipairs(&names) {
            let m = tbl(
                vec![
                    (s("name"), concat(&name, &suffix)),
                    (
                        s("type"),
                        if let Value::Table(_) = mod_type {
                            index(&mod_type, &i)
                        } else {
                            mod_type.clone()
                        },
                    ),
                    (
                        s("value"),
                        if let Value::Table(_) = mod_value {
                            index(&mod_value, &i)
                        } else {
                            mod_value.clone()
                        },
                    ),
                    (s("flags"), flags.clone()),
                    (s("keywordFlags"), keyword_flags.clone()),
                ],
                tag_list.array(),
            );
            if let Some(l) = &mod_tag_list {
                let sub = l.get(&i);
                if sub.truthy() {
                    // t_insert(modList[i], unpack(modTagList[i])): with one
                    // value appends it, with none Lua raises an error, with
                    // several the first is the position
                    let vals = unpack(&sub);
                    match vals.len() {
                        1 => t_insert(&m, vals[0].clone()),
                        0 => panic!("wrong number of arguments to 'insert'"),
                        _ => panic!("table.insert with a position in parseMod"),
                    }
                }
            }
            mod_list.set(i, m);
        }
        let mod_list = Value::Table(mod_list);
        if index(&mod_list, &n(1.0)).truthy() {
            let misc_v = Value::Table(misc.clone());
            let mf = |k: &str| field(&misc_v, k);
            if mf("addToAura").truthy() {
                for (i, effect_mod) in ipairs(&mod_list) {
                    let v = tbl(vec![(s("mod"), effect_mod)], vec![]);
                    let new_mod = if mf("onlyAddToBanners").truthy() {
                        let tag = tbl(
                            vec![
                                (s("type"), s("SkillType")),
                                (s("skillType"), ctx.data.skill_type("Banner")),
                            ],
                            vec![],
                        );
                        first(ctx.mod_(vec![s("ExtraAuraEffect"), s("LIST"), v, tag]))
                    } else {
                        first(ctx.mod_(vec![s("ExtraAuraEffect"), s("LIST"), v]))
                    };
                    set_index(&mod_list, i, new_mod);
                }
            } else if mf("newAura").truthy() {
                for (i, effect_mod) in ipairs(&mod_list) {
                    let tags = ipairs(&effect_mod);
                    for (j, _) in &tags {
                        set_index(&effect_mod, j.clone(), nil());
                    }
                    let v = tbl(
                        vec![
                            (s("mod"), effect_mod.clone()),
                            (s("onlyAllies"), mf("newAuraOnlyAllies")),
                        ],
                        vec![],
                    );
                    let mut args = vec![s("ExtraAura"), s("LIST"), v];
                    args.extend(tags.into_iter().map(|(_, t)| t));
                    set_index(&mod_list, i, first(ctx.mod_(args)));
                }
            } else if mf("addToMinion").truthy() {
                for (i, effect_mod) in ipairs(&mod_list) {
                    let mut tags = Vec::new();
                    if mf("playerTag").truthy() {
                        tags.push(mf("playerTag"));
                    }
                    if mf("addToMinionTag").truthy() {
                        tags.push(mf("addToMinionTag"));
                    }
                    if mf("playerTagList").truthy() {
                        tags.extend(ipairs(&mf("playerTagList")).into_iter().map(|(_, t)| t));
                    }
                    let mut args = vec![
                        s("MinionModifier"),
                        s("LIST"),
                        tbl(vec![(s("mod"), effect_mod)], vec![]),
                    ];
                    args.extend(tags);
                    set_index(&mod_list, i, first(ctx.mod_(args)));
                }
            } else if mf("addToSkill").truthy() {
                for (i, effect_mod) in ipairs(&mod_list) {
                    let args = vec![
                        s("ExtraSkillMod"),
                        s("LIST"),
                        tbl(vec![(s("mod"), effect_mod)], vec![]),
                        mf("addToSkill"),
                    ];
                    set_index(&mod_list, i, first(ctx.mod_(args)));
                }
            } else if mf("applyToEnemy").truthy() {
                for (i, effect_mod) in ipairs(&mod_list) {
                    let mut tags = Vec::new();
                    if mf("playerTag").truthy() {
                        tags.push(mf("playerTag"));
                    }
                    if mf("playerTagList").truthy() {
                        tags.extend(ipairs(&mf("playerTagList")).into_iter().map(|(_, t)| t));
                    }
                    let mut new_mod = effect_mod.clone();
                    if index(&effect_mod, &n(1.0)).truthy() && mf("actorEnemy").truthy() {
                        new_mod = copy_table(&effect_mod);
                        set_index(&index(&new_mod, &n(1.0)), s("actor"), s("enemy"));
                    }
                    let mut args = vec![
                        s("EnemyModifier"),
                        s("LIST"),
                        tbl(vec![(s("mod"), new_mod)], vec![]),
                    ];
                    args.extend(tags);
                    set_index(&mod_list, i, first(ctx.mod_(args)));
                }
            }
        }
        let rest = if first(str_match(&line, &s("%S"), &nil())).truthy() {
            line
        } else {
            nil()
        };
        (mod_list, rest)
    }
}

/// Calls a jewel function entry with captures: `patternVal.func(cap1, ...)`.
/// The result is a closure the calcs run.
fn jewel_func_call(entry: &Value, pattern: &LuaStr, caps: &[Value]) -> Value {
    let mut upvalues = vec![Value::Str(pattern.clone())];
    upvalues.extend(caps.iter().cloned());
    let _ = entry;
    closure("jewelFunc", upvalues)
}

fn lua_str(v: &Value) -> LuaStr {
    v.as_str().expect("string").clone()
}

/// Copy of a parse result, as `copyTable(cache[line])` makes.
fn copy_value(v: &Value) -> Value {
    match v {
        Value::Table(t) => Value::Table(t.deep_copy()),
        v => v.clone(),
    }
}

impl Ctx {
    /// The gem loop of ModParser.lua: skill name lists and the gem-specific
    /// special mods.
    fn add_gem_patterns(
        &self,
        skill_name_list: &mut List,
        pre_skill_name_list: &mut List,
        special: &mut List,
    ) {
        let gems = self.data.get("gems");
        let skill_type = |n: &str| self.data.skill_type(n);
        for (_, gem) in pairs(&gems) {
            let ge = field(&gem, "grantedEffect");
            if field(&ge, "hidden").truthy() || field(&ge, "support").truthy() {
                continue;
            }
            let skill_name = field(&ge, "name");
            let lname = lower(&skill_name);
            let lname_s = lname.as_str().expect("name").to_string();
            let name_tag = || {
                tbl(
                    vec![
                        (s("type"), s("SkillName")),
                        (s("skillName"), skill_name.clone()),
                        (s("includeTransfigured"), Value::Bool(true)),
                    ],
                    vec![],
                )
            };
            let tag_entry = || Entry::Value(tbl(vec![(s("tag"), name_tag())], vec![]));
            let buff_entry = || {
                Entry::Value(tbl(
                    vec![
                        (s("addToSkill"), name_tag()),
                        (
                            s("tag"),
                            tbl(
                                vec![(s("type"), s("GlobalEffect")), (s("effectType"), s("Buff"))],
                                vec![],
                            ),
                        ),
                    ],
                    vec![],
                ))
            };
            let key = |k: String| LuaStr::from(k);
            skill_name_list.set(key(format!(" {lname_s} ")), tag_entry());
            pre_skill_name_list.set(key(format!("^{lname_s} ")), tag_entry());
            pre_skill_name_list.set(key(format!("^{lname_s} has ?a? ")), tag_entry());
            pre_skill_name_list.set(key(format!("^{lname_s} deals ")), tag_entry());
            pre_skill_name_list.set(key(format!("^{lname_s} damage ")), tag_entry());
            let tags = field(&gem, "tags");
            let has = |t: &str| field(&tags, t).truthy();
            if has("totem") {
                pre_skill_name_list.set(key(format!("^{lname_s} totem deals ")), tag_entry());
                pre_skill_name_list.set(key(format!("^{lname_s} totem grants ")), buff_entry());
            }
            if index(&field(&ge, "skillTypes"), &skill_type("Buff")).truthy()
                || field(&field(&ge, "baseFlags"), "buff").truthy()
            {
                pre_skill_name_list.set(key(format!("^{lname_s} grants ")), buff_entry());
                pre_skill_name_list.set(
                    key(format!("^{lname_s} grants a?n? ?additional ")),
                    buff_entry(),
                );
            }
            if has("aura") || has("herald") {
                let var = concat(
                    &s("AffectedBy"),
                    &first(gsub(&skill_name, &s(" "), &s(""), &nil())),
                );
                let cond = || {
                    Entry::Value(tbl(
                        vec![(
                            s("tag"),
                            tbl(
                                vec![(s("type"), s("Condition")), (s("var"), var.clone())],
                                vec![],
                            ),
                        )],
                        vec![],
                    ))
                };
                skill_name_list.set(key(format!("while affected by {lname_s}")), cond());
                skill_name_list.set(key(format!("while using {lname_s}")), cond());
            }
            if has("curse") {
                let v = first(gsub_fn(&skill_name, &s("^%l"), &mut upper_fn, &nil()));
                let v = first(gsub_fn(&v, &s(" %l"), &mut upper_fn, &nil()));
                let v = first(gsub(&v, &s(" "), &s(""), &nil()));
                let v = first(gsub(&v, &s(" "), &s(""), &nil()));
                let var = concat(&s("SelfCast"), &v);
                let entry = Entry::Value(tbl(
                    vec![(
                        s("tag"),
                        tbl(vec![(s("type"), s("Condition")), (s("var"), var)], vec![]),
                    )],
                    vec![],
                ));
                skill_name_list.set(
                    key(format!(
                        "if you've cast {lname_s} in the past (%d+) seconds"
                    )),
                    entry,
                );
            }
            // Closures capturing the skill name
            let sn = skill_name.clone();
            let extra_skill_mod = move |c: &Ctx, inner: Value| -> Value {
                let tag = tbl(
                    vec![
                        (s("type"), s("SkillName")),
                        (s("skillName"), sn.clone()),
                        (s("includeTransfigured"), Value::Bool(true)),
                    ],
                    vec![],
                );
                first(c.mod_(vec![
                    s("ExtraSkillMod"),
                    s("LIST"),
                    tbl(vec![(s("mod"), inner)], vec![]),
                    tag,
                ]))
            };
            let esm = Rc::new(extra_skill_mod);
            let func = |f: ValueFn| -> Entry {
                Entry::Func(Rc::new(move |c: &Ctx, a: &[Value]| vec![f(c, a)]))
            };
            if has("mine") {
                let esm = esm.clone();
                special.set(
                    key(format!("^{lname_s} has (%d+)%% increased throwing speed")),
                    func(Rc::new(move |c, a| {
                        tbl(
                            vec![],
                            vec![esm(
                                c,
                                first(c.mod_(vec![s("MineLayingSpeed"), s("INC"), arg(a, 0)])),
                            )],
                        )
                    })),
                );
            }
            if has("trap") {
                let esm = esm.clone();
                special.set(
                    key(format!("(%d+)%% increased {lname_s} throwing speed")),
                    func(Rc::new(move |c, a| {
                        tbl(
                            vec![],
                            vec![esm(
                                c,
                                first(c.mod_(vec![s("TrapThrowingSpeed"), s("INC"), arg(a, 0)])),
                            )],
                        )
                    })),
                );
            }
            if has("chaining") {
                special.set(
                    key(format!("^{lname_s} chains an additional time")),
                    Entry::Value(tbl(
                        vec![],
                        vec![esm(
                            self,
                            first(self.mod_(vec![s("ChainCountMax"), s("BASE"), n(1.0)])),
                        )],
                    )),
                );
                for pat in [
                    format!("^{lname_s} chains an additional (%d+) times"),
                    format!("^{lname_s} chains (%d+) additional times"),
                ] {
                    let esm = esm.clone();
                    special.set(
                        key(pat),
                        func(Rc::new(move |c, a| {
                            tbl(
                                vec![],
                                vec![esm(
                                    c,
                                    first(c.mod_(vec![s("ChainCountMax"), s("BASE"), arg(a, 0)])),
                                )],
                            )
                        })),
                    );
                }
            }
            if has("bow") {
                let arrow = field(&self.data.global("KeywordFlag"), "Arrow");
                let esm1 = esm.clone();
                let arrow1 = arrow.clone();
                special.set(
                    key(format!("^{lname_s} fires an additional arrow")),
                    func(Rc::new(move |c, _a| {
                        tbl(
                            vec![],
                            vec![esm1(
                                c,
                                first(c.mod_(vec![
                                    s("ProjectileCount"),
                                    s("BASE"),
                                    n(1.0),
                                    nil(),
                                    n(0.0),
                                    arrow1.clone(),
                                ])),
                            )],
                        )
                    })),
                );
                let esm2 = esm.clone();
                special.set(
                    key(format!("^{lname_s} fires (%d+) additional arrows?")),
                    func(Rc::new(move |c, a| {
                        tbl(
                            vec![],
                            vec![esm2(
                                c,
                                first(c.mod_(vec![
                                    s("ProjectileCount"),
                                    s("BASE"),
                                    arg(a, 0),
                                    nil(),
                                    n(0.0),
                                    arrow.clone(),
                                ])),
                            )],
                        )
                    })),
                );
            }
            if has("projectile") {
                special.set(
                    key(format!("^{lname_s} pierces an additional target")),
                    Entry::Value(tbl(
                        vec![],
                        vec![first(self.mod_(vec![
                            s("PierceCount"),
                            s("BASE"),
                            n(1.0),
                            name_tag(),
                        ]))],
                    )),
                );
                let sn = skill_name.clone();
                special.set(
                    key(format!("^{lname_s} pierces (%d+) additional targets?")),
                    func(Rc::new(move |c, a| {
                        let tag = tbl(
                            vec![
                                (s("type"), s("SkillName")),
                                (s("skillName"), sn.clone()),
                                (s("includeTransfigured"), Value::Bool(true)),
                            ],
                            vec![],
                        );
                        tbl(
                            vec![],
                            vec![first(c.mod_(vec![
                                s("PierceCount"),
                                s("BASE"),
                                arg(a, 0),
                                tag,
                            ]))],
                        )
                    })),
                );
            }
            if has("bow") || has("projectile") {
                special.set(
                    key(format!("^{lname_s} fires an additional projectile")),
                    Entry::Value(tbl(
                        vec![],
                        vec![esm(
                            self,
                            first(self.mod_(vec![s("ProjectileCount"), s("BASE"), n(1.0)])),
                        )],
                    )),
                );
                for pat in [
                    format!("^{lname_s} fires (%d+) additional projectiles"),
                    format!("^{lname_s} fires (%d+) additional shard projectiles"),
                ] {
                    let esm = esm.clone();
                    special.set(
                        key(pat),
                        func(Rc::new(move |c, a| {
                            tbl(
                                vec![],
                                vec![esm(
                                    c,
                                    first(c.mod_(vec![s("ProjectileCount"), s("BASE"), arg(a, 0)])),
                                )],
                            )
                        })),
                    );
                }
            }
        }
    }

    /// The unified `jewelFuncList`.
    fn jewel_func_list(
        &self,
        other: &Value,
        self_funcs: &Value,
        unalloc: &Value,
        threshold: &Value,
    ) -> List {
        let mut list = List::default();
        let entry = |func: Value, typ: &str| {
            Entry::Value(tbl(vec![(s("func"), func), (s("type"), s(typ))], vec![]))
        };
        for (k, v) in pairs(other) {
            list.set(
                lua_str(&lower(&k)),
                entry(closure("jewelOtherWrapper", vec![v]), "Other"),
            );
        }
        for (k, v) in pairs(self_funcs) {
            list.set(lua_str(&lower(&k)), entry(v, "Self"));
        }
        for (k, v) in pairs(unalloc) {
            list.set(lua_str(&lower(&k)), entry(v, "SelfUnalloc"));
        }
        for (k, v) in pairs(threshold) {
            let func = if let Value::Table(_) = v {
                closure("jewelThresholdMulti", vec![v])
            } else {
                v
            };
            list.set(lua_str(&lower(&k)), entry(func, "Threshold"));
        }
        list
    }

    fn cluster_jewel_skills(&self, cluster_jewels: &Value) -> List {
        let mut list = List::default();
        for (_, jewel) in pairs(&field(cluster_jewels, "jewels")) {
            for (skill_id, skill) in pairs(&field(&jewel, "skills")) {
                let enchant: Vec<Vec<u8>> = ipairs(&field(&skill, "enchant"))
                    .into_iter()
                    .map(|(_, l)| l.as_str().expect("enchant").as_bytes().to_vec())
                    .collect();
                let k = lower(&sb(&enchant.join(&b" "[..])));
                let v = tbl(
                    vec![(s("key"), s("clusterJewelSkill")), (s("value"), skill_id)],
                    vec![],
                );
                list.set(
                    lua_str(&k),
                    Entry::Value(tbl(
                        vec![],
                        vec![first(self.mod_(vec![s("JewelData"), s("LIST"), v]))],
                    )),
                );
            }
        }
        for (notable, _) in pairs(&field(cluster_jewels, "notableSortOrder")) {
            let k = concat(&s("1 added passive skill is "), &lower(&notable));
            list.set(
                lua_str(&k),
                Entry::Value(tbl(
                    vec![],
                    vec![first(self.mod_(vec![
                        s("ClusterJewelNotable"),
                        s("LIST"),
                        notable,
                    ]))],
                )),
            );
        }
        for (_, keystone) in ipairs(&field(cluster_jewels, "keystones")) {
            let k = concat(&s("adds "), &lower(&keystone));
            let v = tbl(
                vec![
                    (s("key"), s("clusterJewelKeystone")),
                    (s("value"), keystone),
                ],
                vec![],
            );
            list.set(
                lua_str(&k),
                Entry::Value(tbl(
                    vec![],
                    vec![first(self.mod_(vec![s("JewelData"), s("LIST"), v]))],
                )),
            );
        }
        list
    }
}
