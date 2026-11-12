//! The mod parser's typed tables (`engine::mod_parser::rules`), made from
//! the importer's evaluation of upstream's `ModParser.lua` with the setup
//! code around its tables (the gem loop, keystones, jewel functions, cluster
//! jewel skills, resource types), and upstream's `Data/ModCache.lua`.

use engine::data::Data;
use engine::mod_parser::rules::{Closure, FlagType, GemEffects, JewelFunc, Secondary, SupportGem};
use engine::mod_parser::{Entry, Form, Frag, GemKind, InitError, List, Parsed, ParserData, Rules};
use engine::mods::Mod;
use engine::mods::convert::Converter;
use engine::rt::*;
use lua::modlib::create_mod_args;
use lua::{Key, TableRef, Value};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

type R<T> = std::result::Result<T, InitError>;

fn bad(e: impl std::fmt::Display) -> InitError {
    InitError::Table(e.to_string())
}

fn text(v: &Value) -> R<String> {
    v.as_str()
        .map(|s| s.to_str_lossy().into_owned())
        .ok_or_else(|| bad(format!("expected a string, got {v:?}")))
}

fn mod_(args: Vec<Value>) -> Value {
    Value::Table(create_mod_args(&args))
}

fn flag(name: Value) -> Value {
    mod_(vec![name, s("FLAG"), Value::Bool(true)])
}

fn first_to_upper(v: &Value) -> Value {
    first(gsub_fn(v, &s("^%l"), &mut upper_fn, &nil()))
}

/// A pattern table: values converted by `conv`, closures by their ports
fn list<T>(t: &Value, conv: impl Fn(&Value) -> R<T>) -> R<List<T>> {
    let mut out = List::default();
    for (k, v) in t.as_table().ok_or_else(|| bad("not a table"))?.entries() {
        let key = match k {
            Key::Str(s) => s.to_str_lossy().into_owned(),
            k => return Err(bad(format!("pattern key {k:?}"))),
        };
        let entry = match &v {
            Value::Func(f) => Entry::Func(Closure::new(&f.key, f.source_hash)?),
            v => Entry::Value(conv(v)?),
        };
        out.set(key, entry);
    }
    Ok(out)
}

fn frag(v: &Value) -> R<Frag> {
    engine::mod_parser::frag(v).map_err(bad)
}

fn mods(v: &Value) -> R<Vec<Mod>> {
    let conv = Converter::default();
    let t = v
        .as_table()
        .ok_or_else(|| bad("a mod list is not a table"))?;
    let arr = t.array();
    if arr.len() != t.borrow().len_pairs() {
        return Err(bad("a mod list with other keys"));
    }
    arr.iter().map(|m| conv.mod_(m).map_err(bad)).collect()
}

fn value(v: &Value) -> R<engine::mods::ModValue> {
    Converter::default().value(v).map_err(bad)
}

fn name(v: &Value) -> R<Arc<str>> {
    Ok(text(v)?.into())
}

/// The typed tables of `ModParser.lua`, as its setup code leaves them.
/// Closures without a port, or whose source changed since, are errors.
pub fn rules(data: &Data, tables: &TableRef) -> R<Rules> {
    let t = |name: &str| tables.get_str(name);
    let gem_id_lookup = t("gemIdLookup");

    // gemIdLookup: every non-hidden skill, plus hidden ones granted by items or the tree
    let skills = data.get("skills");
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

    let mut special = list(&t("specialModList"), mods)?;
    for (_, name) in ipairs(&data.get("keystones")) {
        let have = first_to_upper(&name);
        let have = first(gsub_fn(&have, &s(" %l"), &mut upper_fn, &nil()));
        let have = first(gsub(&have, &s(" "), &s(""), &nil()));
        let m = tbl(
            vec![],
            vec![
                mod_(vec![s("Keystone"), s("LIST"), name.clone()]),
                flag(concat(&s("Condition:Have"), &have)),
            ],
        );
        special.set(text(&lower(&name))?, Entry::Value(mods(&m)?));
    }
    let cluster_jewels = data.get("clusterJewels");
    for (_, name) in ipairs(&field(&cluster_jewels, "keystones")) {
        let m = tbl(
            vec![],
            vec![mod_(vec![s("Keystone"), s("LIST"), name.clone()])],
        );
        special.set(text(&lower(&name))?, Entry::Value(mods(&m)?));
    }
    let mut anchored = List::default();
    for (k, v) in special.patterns.into_iter().zip(special.entries) {
        anchored.set(format!("^{k}$"), v);
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
        Value::Table(out)
    };
    let map = |v: &Value| -> R<HashMap<String, Frag>> {
        let l = list(v, frag)?;
        l.patterns
            .into_iter()
            .zip(l.entries)
            .map(|(k, e)| match e {
                Entry::Value(f) => Ok((k, f)),
                _ => Err(bad("a resource type function")),
            })
            .collect()
    };
    let regen_types = map(&append_mod("Regen"))?;
    let degen_types = map(&append_mod("Degen"))?;
    let cost_types = list(&append_mod("Cost"), frag)?;
    let base_cost_types = list(&append_mod("CostNoMult"), frag)?;

    let mut skill_name_list = list(&t("skillNameList"), frag)?;
    let mut pre_skill_name_list = List::default();
    add_gem_patterns(
        data,
        &mut skill_name_list,
        &mut pre_skill_name_list,
        &mut special,
    )?;

    let dmg_types = pairs(&t("dmgTypes"))
        .into_iter()
        .map(|(k, v)| Ok((text(&k)?, name(&v)?)))
        .collect::<R<_>>()?;
    let unsupported_mod_list = pairs(&t("unsupportedModList"))
        .into_iter()
        .map(|(k, _)| text(&k))
        .collect::<R<_>>()?;

    let mut upvalues = Vec::new();
    for g in ["ModFlag", "KeywordFlag", "SkillType"] {
        upvalues.push((g.to_string(), value(&data.global(g))?));
    }
    for n in ["damageTypeList", "conquerorList", "dmgTypes", "gemIdLookup"] {
        upvalues.push((n.to_string(), value(&t(n))?));
    }
    let data_fields = TableRef::new();
    for f in engine::mod_parser::DATA_FIELDS {
        let v = data.get(f);
        if v.is_nil() {
            return Err(bad(format!("data.{f} is missing")));
        }
        data_fields.set_str(f, v);
    }
    upvalues.push(("data".into(), value(&Value::Table(data_fields))?));

    Ok(Rules {
        form_list: list(&t("formList"), |v| {
            let n = text(v)?;
            Form::new(&n).ok_or_else(|| bad(format!("unknown mod form {n}")))
        })?,
        mod_name_list: list(&t("modNameList"), frag)?,
        mod_flag_list: list(&t("modFlagList"), frag)?,
        pre_flag_list: list(&t("preFlagList"), frag)?,
        mod_tag_list: list(&t("modTagList"), frag)?,
        special_mod_list: special,
        unsupported_mod_list,
        suffix_types: list(&t("suffixTypes"), name)?,
        dmg_types,
        pen_types: list(&t("penTypes"), frag)?,
        flag_types: list(&t("flagTypes"), |v| match v {
            Value::Str(_) => Ok(FlagType::Name(name(v)?)),
            _ => Ok(FlagType::Mod {
                name: name(&field(v, "name"))?,
                typ: name(&field(v, "type"))?,
                value: value(&field(v, "value"))?,
            }),
        })?,
        regen_types,
        degen_types,
        cost_types,
        base_cost_types,
        skill_name_list,
        pre_skill_name_list,
        jewel_func_list: jewel_func_list(
            &t("jewelOtherFuncs"),
            &t("jewelSelfFuncs"),
            &t("jewelSelfUnallocFuncs"),
            &t("jewelThresholdFuncs"),
        )?,
        cluster_jewel_skills: cluster_jewel_skills(&cluster_jewels)?,
        support_gems: support_gems(data, &gem_id_lookup)?,
        upvalues,
    })
}

/// What `extraSupport` reads of each skill `gemIdLookup` gives
fn support_gems(data: &Data, lookup: &Value) -> R<HashMap<String, SupportGem>> {
    let mut out = HashMap::new();
    let opt = |v: Value| -> R<Option<engine::mods::ModValue>> {
        if v.is_nil() {
            Ok(None)
        } else {
            value(&v).map(Some)
        }
    };
    for (_, id) in pairs(lookup) {
        let skill_name = field(&index(&data.get("skills"), &id), "name");
        if skill_name.is_nil() {
            continue;
        }
        let gem_id = index(
            &data.get("gemForBaseName"),
            &lower(&concat(&skill_name, &s(" Support"))),
        );
        let gem = if gem_id.truthy() {
            let gem = index(&data.get("gems"), &gem_id);
            let secondary = field(&gem, "secondaryGrantedEffect");
            let secondary = if secondary.truthy() {
                Some(Secondary {
                    id: opt(field(&gem, "secondaryGrantedEffectId"))?,
                    support: field(&secondary, "support").truthy(),
                })
            } else {
                None
            };
            Some(GemEffects {
                granted: opt(field(&gem, "grantedEffectId"))?,
                secondary,
            })
        } else {
            None
        };
        out.insert(text(&id)?, SupportGem { gem });
    }
    Ok(out)
}

/// The gem loop of ModParser.lua: skill name lists and the gem-specific
/// special mods.
fn add_gem_patterns(
    data: &Data,
    skill_name_list: &mut List<Frag>,
    pre_skill_name_list: &mut List<Frag>,
    special: &mut List<Vec<Mod>>,
) -> R<()> {
    let gems = data.get("gems");
    let skill_type = |n: &str| data.skill_type(n);
    for (_, gem) in pairs(&gems) {
        let ge = field(&gem, "grantedEffect");
        if field(&ge, "hidden").truthy() || field(&ge, "support").truthy() {
            continue;
        }
        let skill_name = field(&ge, "name");
        let lname = text(&lower(&skill_name))?;
        let sname: Arc<str> = text(&skill_name)?.into();
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
        let tag_entry = || frag(&tbl(vec![(s("tag"), name_tag())], vec![])).map(Entry::Value);
        let buff_entry = || {
            frag(&tbl(
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
            .map(Entry::Value)
        };
        skill_name_list.set(format!(" {lname} "), tag_entry()?);
        pre_skill_name_list.set(format!("^{lname} "), tag_entry()?);
        pre_skill_name_list.set(format!("^{lname} has ?a? "), tag_entry()?);
        pre_skill_name_list.set(format!("^{lname} deals "), tag_entry()?);
        pre_skill_name_list.set(format!("^{lname} damage "), tag_entry()?);
        let tags = field(&gem, "tags");
        let has = |t: &str| field(&tags, t).truthy();
        if has("totem") {
            pre_skill_name_list.set(format!("^{lname} totem deals "), tag_entry()?);
            pre_skill_name_list.set(format!("^{lname} totem grants "), buff_entry()?);
        }
        if index(&field(&ge, "skillTypes"), &skill_type("Buff")).truthy()
            || field(&field(&ge, "baseFlags"), "buff").truthy()
        {
            pre_skill_name_list.set(format!("^{lname} grants "), buff_entry()?);
            pre_skill_name_list.set(format!("^{lname} grants a?n? ?additional "), buff_entry()?);
        }
        if has("aura") || has("herald") {
            let var = concat(
                &s("AffectedBy"),
                &first(gsub(&skill_name, &s(" "), &s(""), &nil())),
            );
            let cond = || {
                frag(&tbl(
                    vec![(
                        s("tag"),
                        tbl(
                            vec![(s("type"), s("Condition")), (s("var"), var.clone())],
                            vec![],
                        ),
                    )],
                    vec![],
                ))
                .map(Entry::Value)
            };
            skill_name_list.set(format!("while affected by {lname}"), cond()?);
            skill_name_list.set(format!("while using {lname}"), cond()?);
        }
        if has("curse") {
            let v = first(gsub_fn(&skill_name, &s("^%l"), &mut upper_fn, &nil()));
            let v = first(gsub_fn(&v, &s(" %l"), &mut upper_fn, &nil()));
            let v = first(gsub(&v, &s(" "), &s(""), &nil()));
            let v = first(gsub(&v, &s(" "), &s(""), &nil()));
            let var = concat(&s("SelfCast"), &v);
            let entry = frag(&tbl(
                vec![(
                    s("tag"),
                    tbl(vec![(s("type"), s("Condition")), (s("var"), var)], vec![]),
                )],
                vec![],
            ))?;
            skill_name_list.set(
                format!("if you've cast {lname} in the past (%d+) seconds"),
                Entry::Value(entry),
            );
        }
        let esm = |inner: Value| -> Value {
            mod_(vec![
                s("ExtraSkillMod"),
                s("LIST"),
                tbl(vec![(s("mod"), inner)], vec![]),
                name_tag(),
            ])
        };
        let one = |m: Value| mods(&tbl(vec![], vec![m])).map(Entry::Value);
        let gem_rule = |k: GemKind| Entry::Gem(k, sname.clone());
        if has("mine") {
            special.set(
                format!("^{lname} has (%d+)%% increased throwing speed"),
                gem_rule(GemKind::MineThrowingSpeed),
            );
        }
        if has("trap") {
            special.set(
                format!("(%d+)%% increased {lname} throwing speed"),
                gem_rule(GemKind::TrapThrowingSpeed),
            );
        }
        if has("chaining") {
            special.set(
                format!("^{lname} chains an additional time"),
                one(esm(mod_(vec![s("ChainCountMax"), s("BASE"), n(1.0)])))?,
            );
            for pat in [
                format!("^{lname} chains an additional (%d+) times"),
                format!("^{lname} chains (%d+) additional times"),
            ] {
                special.set(pat, gem_rule(GemKind::Chains));
            }
        }
        if has("bow") {
            special.set(
                format!("^{lname} fires an additional arrow"),
                gem_rule(GemKind::FiresArrow),
            );
            special.set(
                format!("^{lname} fires (%d+) additional arrows?"),
                gem_rule(GemKind::FiresArrows),
            );
        }
        if has("projectile") {
            special.set(
                format!("^{lname} pierces an additional target"),
                one(mod_(vec![s("PierceCount"), s("BASE"), n(1.0), name_tag()]))?,
            );
            special.set(
                format!("^{lname} pierces (%d+) additional targets?"),
                gem_rule(GemKind::Pierces),
            );
        }
        if has("bow") || has("projectile") {
            special.set(
                format!("^{lname} fires an additional projectile"),
                one(esm(mod_(vec![s("ProjectileCount"), s("BASE"), n(1.0)])))?,
            );
            for pat in [
                format!("^{lname} fires (%d+) additional projectiles"),
                format!("^{lname} fires (%d+) additional shard projectiles"),
            ] {
                special.set(pat, gem_rule(GemKind::FiresProjectiles));
            }
        }
    }
    Ok(())
}

fn closure(name: &str, upvalues: Vec<Value>) -> Value {
    Value::Closure(Rc::new(lua::Closure {
        name: name.into(),
        upvalues,
    }))
}

/// The unified `jewelFuncList`.
fn jewel_func_list(
    other: &Value,
    self_funcs: &Value,
    unalloc: &Value,
    threshold: &Value,
) -> R<List<JewelFunc>> {
    let mut out = List::default();
    let mut add = |k: &Value, func: Value, typ: &str| -> R<()> {
        let v = tbl(vec![(s("func"), func), (s("type"), s(typ))], vec![]);
        let jf = JewelFunc {
            typ: typ.into(),
            value: value(&v)?,
        };
        out.set(text(&lower(k))?, Entry::Value(jf));
        Ok(())
    };
    for (k, v) in pairs(other) {
        add(&k, closure("jewelOtherWrapper", vec![v]), "Other")?;
    }
    for (k, v) in pairs(self_funcs) {
        add(&k, v, "Self")?;
    }
    for (k, v) in pairs(unalloc) {
        add(&k, v, "SelfUnalloc")?;
    }
    for (k, v) in pairs(threshold) {
        let func = if let Value::Table(_) = v {
            closure("jewelThresholdMulti", vec![v])
        } else {
            v
        };
        add(&k, func, "Threshold")?;
    }
    Ok(out)
}

fn cluster_jewel_skills(cluster_jewels: &Value) -> R<HashMap<String, Vec<Mod>>> {
    let mut out = HashMap::new();
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
            let m = tbl(vec![], vec![mod_(vec![s("JewelData"), s("LIST"), v])]);
            out.insert(text(&k)?, mods(&m)?);
        }
    }
    for (notable, _) in pairs(&field(cluster_jewels, "notableSortOrder")) {
        let k = concat(&s("1 added passive skill is "), &lower(&notable));
        let m = tbl(
            vec![],
            vec![mod_(vec![s("ClusterJewelNotable"), s("LIST"), notable])],
        );
        out.insert(text(&k)?, mods(&m)?);
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
        let m = tbl(vec![], vec![mod_(vec![s("JewelData"), s("LIST"), v])]);
        out.insert(text(&k)?, mods(&m)?);
    }
    Ok(out)
}

/// `Data/ModCache.lua` (line -> `{ modList, extra }`), typed as
/// `copyTable(cache[line])`
pub fn mod_cache(cache: &TableRef) -> R<HashMap<String, Parsed>> {
    let mut out = HashMap::new();
    for (line, entry) in cache.entries() {
        let line = text(&line.to_value())?;
        // What `parseMod` returns: a copy, whose `pairs` order is the copy's
        let entry = copy_table(&entry);
        let list = index(&entry, &n(1.0));
        let mods = match list {
            Value::Nil => None,
            l => Some(mods(&l)?),
        };
        let extra = match index(&entry, &n(2.0)) {
            Value::Nil => None,
            e => Some(text(&e)?),
        };
        out.insert(line, (mods, extra));
    }
    Ok(out)
}

/// The parser's typed data from upstream's `src/` directory
pub fn parser_data(src_dir: &std::path::Path) -> Result<ParserData, String> {
    let raw = importer::import(src_dir).map_err(|e| e.to_string())?;
    let data = Data::new(raw.globals.clone(), raw.data.clone());
    let mut im = importer::Importer::new(src_dir);
    let tables = importer::mod_parser::import(&mut im, &raw).map_err(|e| e.to_string())?;
    let rules = rules(&data, &tables).map_err(|e| e.to_string())?;
    let cache = importer::mod_parser::import_mod_cache(&mut im).map_err(|e| e.to_string())?;
    let cache = mod_cache(&cache).map_err(|e| e.to_string())?;
    Ok(ParserData { rules, cache })
}
