//! What closures reach of ModParser.lua's module: its helper functions
//! (`mod`, `flag`, `extraSupport`, ...) and values (`ModFlag`,
//! `gemIdLookup`, ...), typed.

use super::rules::{Rules, SupportGem};
use super::val::*;
use crate::mods::convert::Converter;
use crate::mods::{Func, Mod, ModType, ModValue, Sym};
use std::collections::HashMap;
use std::rc::Rc;

pub struct Ctx {
    /// Records by key, lists by position from 1
    upvalues: HashMap<String, HashMap<Vec<u8>, ModValue>>,
    lists: HashMap<String, ModValue>,
    support_gems: HashMap<String, SupportGem>,
}

fn panic_on<T>(what: &str) -> impl Fn(String) -> T + '_ {
    move |e| panic!("ModParser {what}: {e}")
}

/// `modLib.createMod(name, type, value, ...)` as a typed mod. `rest` is
/// Lua's `...`: a source, flags and keyword flags where given, then tags;
/// tags after a nil are kept by index, as upstream's never reaches them.
pub fn make_mod(args: &[V]) -> Mod {
    let conv = Converter::default();
    let rest = args.get(3..).unwrap_or(&[]);
    let sel = |i: usize| rest.get(i - 1).cloned().unwrap_or_default();
    let (mut tag_start, mut source, mut flags, mut keyword_flags) = (1, None, 0u32, 0u32);
    let bits = |x: f64| x.rem_euclid(4294967296.0) as u64 as u32;
    if let V::Str(s) = sel(1) {
        source = Some(String::from_utf8_lossy(&s).into());
        tag_start = 2;
    }
    if let V::Num(x) = sel(2) {
        flags = bits(x);
        tag_start = 3;
    }
    if let V::Num(x) = sel(3) {
        keyword_flags = bits(x);
        tag_start = 4;
    }
    let (mut tags, mut unreached_tags) = (Vec::new(), Vec::new());
    let mut gap = false;
    for (i, t) in rest.iter().skip(tag_start - 1).enumerate() {
        if t.is_nil() {
            gap = true;
            continue;
        }
        let tag = conv.tag(t).unwrap_or_else(panic_on("tag"));
        if gap {
            unreached_tags.push((i + 1, tag));
        } else {
            tags.push(tag);
        }
    }
    let text = |v: &V, what: &str| match v {
        V::Str(s) => String::from_utf8_lossy(s).into_owned(),
        v => panic!("ModParser mod {what} is a {} value", v.type_name()),
    };
    Mod {
        name: Sym::new(&text(&arg(args, 0), "name")),
        typ: ModType::new(&text(&arg(args, 1), "type")),
        value: conv
            .value(&arg(args, 2))
            .unwrap_or_else(panic_on("mod value")),
        flags,
        keyword_flags,
        source,
        source_slot: None,
        tags: tags.into(),
        unreached_tags,
    }
}

fn func(name: &str, args: Vec<V>) -> V {
    let conv = Converter::default();
    V::Func(Rc::new(Func {
        name: name.into(),
        args: args
            .iter()
            .map(|a| (!a.is_nil()).then(|| conv.value(a).unwrap_or_else(panic_on("closure value"))))
            .collect(),
    }))
}

impl Ctx {
    pub fn new(rules: &Rules) -> Ctx {
        let mut upvalues = HashMap::new();
        let mut lists = HashMap::new();
        for (name, v) in &rules.upvalues {
            if let ModValue::Record(r) = v {
                let map = r
                    .fields
                    .iter()
                    .map(|(k, v)| (k.as_str().as_bytes().to_vec(), v.clone()))
                    .collect();
                upvalues.insert(name.clone(), map);
            }
            lists.insert(name.clone(), v.clone());
        }
        Ctx {
            upvalues,
            lists,
            support_gems: rules.support_gems.clone(),
        }
    }

    /// A module value as a whole: `damageTypeList`, ...
    pub fn value(&self, name: &str) -> V {
        match self.lists.get(name) {
            Some(v) => V::from(v.clone()),
            None => panic!("ModParser value {name} is not available"),
        }
    }

    /// `name[key]` of a module value
    pub fn index(&self, name: &str, key: &V) -> V {
        if let Some(map) = self.upvalues.get(name) {
            return match key {
                V::Str(k) => map.get(&k[..]).cloned().map_or(V::Nil, V::from),
                V::Nil => V::Nil,
                _ => index(&self.value(name), key),
            };
        }
        index(&self.value(name), key)
    }

    /// `name.field` of a module value
    pub fn field(&self, name: &str, field: &str) -> V {
        self.index(name, &s(field))
    }

    /// `mod(...)`
    pub fn mod_(&self, args: Vec<V>) -> Vec<V> {
        vec![V::Mod(Rc::new(make_mod(&args)))]
    }

    /// `flag(name, ...)`
    pub fn flag(&self, args: Vec<V>) -> Vec<V> {
        let mut a = vec![arg(&args, 0), s("FLAG"), V::Bool(true)];
        a.extend(args.into_iter().skip(1));
        self.mod_(a)
    }

    /// `firstToUpper(str)`
    pub fn first_to_upper(&self, args: Vec<V>) -> Vec<V> {
        vec![first(gsub_case(&arg(&args, 0), &s("^%l"), true, &nil()))]
    }

    /// `dealNoNonDamageType(dmgType, forMinion)`
    pub fn deal_no_non_damage_type(&self, args: Vec<V>) -> Vec<V> {
        let dmg_type = first(self.first_to_upper(vec![arg(&args, 0)]));
        let for_minion = arg(&args, 1);
        let list = self.value("damageTypeList");
        if !is_value_in_array(&list, &dmg_type) {
            return vec![];
        }
        let mut mods = tbl(vec![], vec![]);
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
                t_insert(&mut mods, m);
            }
        }
        vec![mods]
    }

    /// `grantedExtraSkill(name, level, noSupports)`
    pub fn granted_extra_skill(&self, args: Vec<V>) -> Vec<V> {
        let name = first(gsub(&arg(&args, 0), &s(" skill"), &s(""), &nil()));
        let id = self.index("gemIdLookup", &name);
        if !id.truthy() {
            return vec![];
        }
        let v = tbl(
            vec![
                (s("skillId"), id),
                (s("level"), tonumber(&arg(&args, 1))),
                (s("noSupports"), arg(&args, 2)),
            ],
            vec![],
        );
        vec![tbl(
            vec![],
            vec![first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v]))],
        )]
    }

    /// `triggerExtraSkill(name, level, options)`
    pub fn trigger_extra_skill(&self, args: Vec<V>) -> Vec<V> {
        let options = match arg(&args, 2) {
            o if o.truthy() => o,
            _ => tbl(vec![], vec![]),
        };
        let name = first(gsub(&arg(&args, 0), &s(" skill"), &s(""), &nil()));
        let source = match field(&options, "sourceSkill") {
            x if x.truthy() => first(gsub(&x, &s(" skill"), &s(""), &nil())),
            x => x,
        };
        let mut mods = tbl(vec![], vec![]);
        let id = self.index("gemIdLookup", &name);
        if id.truthy() {
            let v = tbl(
                vec![
                    (s("skillId"), id.clone()),
                    (s("level"), tonumber(&arg(&args, 1))),
                    (s("noSupports"), field(&options, "noSupports")),
                    (s("triggered"), V::Bool(true)),
                    (s("source"), source),
                    (
                        s("triggerChance"),
                        tonumber(&field(&options, "triggerChance")),
                    ),
                ],
                vec![],
            );
            t_insert(
                &mut mods,
                first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v])),
            );
        }
        let skill_tag = || {
            tbl(
                vec![(s("type"), s("SkillId")), (s("skillId"), id.clone())],
                vec![],
            )
        };
        if field(&options, "ignoreHexproof").truthy() {
            let v = tbl(
                vec![(s("key"), s("ignoreHexproof")), (s("value"), V::Bool(true))],
                vec![],
            );
            let m = first(self.mod_(vec![s("SkillData"), s("LIST"), v, skill_tag()]));
            t_insert(&mut mods, m);
        }
        if field(&options, "onCrit").truthy() {
            let inner = tbl(
                vec![(s("key"), s("triggerOnCrit")), (s("value"), V::Bool(true))],
                vec![],
            );
            let inner = first(self.mod_(vec![s("SkillData"), s("LIST"), inner]));
            let v = tbl(vec![(s("mod"), inner)], vec![]);
            let m = first(self.mod_(vec![s("ExtraSkillMod"), s("LIST"), v, skill_tag()]));
            t_insert(&mut mods, m);
        }
        vec![mods]
    }

    /// `extraSupport(name, level, slot)`
    pub fn extra_support(&self, args: Vec<V>) -> Vec<V> {
        let name = arg(&args, 0);
        let mut skill_id = self.index("gemIdLookup", &name);
        if !skill_id.truthy() {
            let n = first(gsub(&name, &s("^increased "), &s(""), &nil()));
            skill_id = self.index("gemIdLookup", &n);
        }
        if !skill_id.truthy() {
            let n = first(gsub(&name, &s(" support$"), &s(""), &nil()));
            skill_id = self.index("gemIdLookup", &n);
        }
        let slot = match arg(&args, 2) {
            x if x.raw_eq(&s("main hand")) || x.raw_eq(&s("main hand weapon")) => s("Weapon 1"),
            x if x.raw_eq(&s("off hand")) || x.raw_eq(&s("off hand weapon")) => s("Weapon 2"),
            x if x.truthy() => {
                let spaced = concat(&s(" "), &x);
                str_sub(
                    &first(gsub_case(&spaced, &s("%W%l"), true, &nil())),
                    &n(2.0),
                    &nil(),
                )
            }
            _ => s("{SlotName}"),
        };
        let level = tonumber(&arg(&args, 1));
        if !skill_id.truthy() {
            return vec![];
        }
        let applies = arg(&args, 3);
        let support_value = |id: V| {
            let mut fields = vec![(s("skillId"), id), (s("level"), level.clone())];
            if !applies.is_nil() {
                fields.push((s("appliesToGrantedSkills"), applies.clone()));
            }
            tbl(fields, vec![])
        };
        let socketed = || {
            tbl(
                vec![(s("type"), s("SocketedIn")), (s("slotName"), slot.clone())],
                vec![],
            )
        };
        let id_text = String::from_utf8_lossy(&to_bytes(&skill_id)).into_owned();
        let support = self.support_gems.get(&id_text).unwrap_or_else(|| {
            panic!("attempt to concatenate the name of unknown skill {id_text}")
        });
        let opt = |v: &Option<ModValue>| v.clone().map_or(V::Nil, V::from);
        let support_mod = |v| first(self.mod_(vec![s("ExtraSupport"), s("LIST"), v, socketed()]));
        let mut mods = tbl(vec![], vec![]);
        match &support.gem {
            Some(g) => {
                t_insert(&mut mods, support_mod(support_value(opt(&g.granted))));
                if let Some(sec) = &g.secondary {
                    if sec.support {
                        t_insert(&mut mods, support_mod(support_value(opt(&sec.id))));
                    } else {
                        let v = tbl(
                            vec![(s("skillId"), opt(&sec.id)), (s("level"), level.clone())],
                            vec![],
                        );
                        t_insert(
                            &mut mods,
                            first(self.mod_(vec![s("ExtraSkill"), s("LIST"), v])),
                        );
                    }
                }
            }
            None => t_insert(&mut mods, support_mod(support_value(skill_id.clone()))),
        }
        vec![mods]
    }

    /// `explodeFunc(chance, amount, type, ...)`
    pub fn explode_func(&self, args: Vec<V>) -> Vec<V> {
        let (chance, amount, typ) = (arg(&args, 0), arg(&args, 1), arg(&args, 2));
        let amount_number = match tonumber(&amount) {
            x if x.truthy() => x,
            _ if amount.raw_eq(&s("tenth")) => n(10.0),
            _ if amount.raw_eq(&s("quarter")) => n(25.0),
            _ => return vec![],
        };
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
    pub fn get_simple_conv(&self, args: Vec<V>) -> Vec<V> {
        vec![func("getSimpleConv", args)]
    }

    /// `getPerStat(...)`, see [`Ctx::get_simple_conv`].
    pub fn get_per_stat(&self, args: Vec<V>) -> Vec<V> {
        vec![func("getPerStat", args)]
    }

    /// `getThreshold(attrib, name, type, value, ...)`: closes over the mod
    /// it builds, `mod(name, type, value, "", ...)`
    pub fn get_threshold(&self, args: Vec<V>) -> Vec<V> {
        let conv = Converter::default();
        let mut mod_args = args.get(1..4).map(<[V]>::to_vec).unwrap_or_default();
        mod_args.resize(3, V::Nil);
        mod_args.push(s(""));
        mod_args.extend(args.iter().skip(4).cloned());
        let attrib = conv
            .value(&arg(&args, 0))
            .unwrap_or_else(panic_on("threshold"));
        vec![V::Func(Rc::new(Func {
            name: "getThreshold".into(),
            args: vec![
                Some(attrib),
                Some(ModValue::Mod(Box::new(make_mod(&mod_args)))),
            ],
        }))]
    }
}

/// A closure value made at parse time: `name` with its upvalues
pub fn closure(name: &str, upvalues: Vec<V>) -> V {
    func(name, upvalues)
}
