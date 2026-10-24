//! Typed mods (`engine::mods`) encoded as the oracle encodes upstream's, as
//! JSON values: to check that converting upstream's mods loses nothing.

use engine::mods::tag::*;
use engine::mods::*;
use serde_json::{Map, Value as J, json};

fn number(n: f64) -> J {
    if n.is_nan() {
        json!({"$num": "nan"})
    } else if n.is_infinite() {
        json!({"$num": if n > 0.0 { "inf" } else { "-inf" }})
    } else {
        serde_json::Number::from_f64(n).map_or(J::Null, J::Number)
    }
}

fn names(m: &mut Map<String, J>, one: &str, list: &str, n: &Option<Names>) {
    match n {
        Some(Names::One(s)) => {
            m.insert(one.into(), json!(s.as_str()));
        }
        Some(Names::List(l)) => {
            m.insert(
                list.into(),
                J::Array(l.iter().map(|s| json!(s.as_str())).collect()),
            );
        }
        None => {}
    }
}

fn opt_num(m: &mut Map<String, J>, k: &str, v: Option<f64>) {
    if let Some(v) = v {
        m.insert(k.into(), number(v));
    }
}

fn opt_sym(m: &mut Map<String, J>, k: &str, v: Option<Sym>) {
    if let Some(v) = v {
        m.insert(k.into(), json!(v.as_str()));
    }
}

fn opt_text(m: &mut Map<String, J>, k: &str, v: &Option<std::rc::Rc<str>>) {
    if let Some(v) = v {
        m.insert(k.into(), json!(&**v));
    }
}

fn yes(m: &mut Map<String, J>, k: &str, v: bool) {
    if v {
        m.insert(k.into(), J::Bool(true));
    }
}

fn nums(v: &[f64]) -> J {
    J::Array(v.iter().map(|n| number(*n)).collect())
}

pub fn value(v: &ModValue) -> J {
    match v {
        ModValue::Bool(b) => J::Bool(*b),
        ModValue::Number(n) => number(*n),
        ModValue::Str(s) => json!(&**s),
        ModValue::Mod(m) => mod_(m),
        ModValue::Record(r) if r.fields.is_empty() => json!({}),
        ModValue::Record(r) => J::Object(
            r.fields
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), value(v)))
                .collect(),
        ),
        ModValue::List(l) if l.is_empty() => json!({}),
        ModValue::List(l) => J::Array(l.iter().map(value).collect()),
        ModValue::Func(_) => json!({"$func": true}),
    }
}

pub fn tag(t: &Tag) -> J {
    let mut m = Map::new();
    let kind = match &t.kind {
        TagKind::Multiplier(x) => {
            names(&mut m, "var", "varList", &x.vars);
            opt_sym(&mut m, "actor", x.actor);
            opt_sym(&mut m, "limitActor", x.limit_actor);
            opt_num(&mut m, "div", x.div);
            opt_sym(&mut m, "divVar", x.div_var);
            yes(&mut m, "noFloor", x.no_floor);
            opt_num(&mut m, "limit", x.limit);
            opt_sym(&mut m, "limitVar", x.limit_var);
            opt_sym(&mut m, "limitStat", x.limit_stat);
            yes(&mut m, "limitTotal", x.limit_total);
            yes(&mut m, "limitNegTotal", x.limit_neg_total);
            yes(&mut m, "invert", x.invert);
            opt_num(&mut m, "base", x.base);
            "Multiplier".to_string()
        }
        TagKind::MultiplierThreshold(x) => {
            names(&mut m, "var", "varList", &x.vars);
            opt_sym(&mut m, "actor", x.actor);
            opt_sym(&mut m, "thresholdActor", x.threshold_actor);
            opt_num(&mut m, "threshold", x.threshold);
            opt_sym(&mut m, "thresholdVar", x.threshold_var);
            yes(&mut m, "upper", x.upper);
            yes(&mut m, "equals", x.equals);
            "MultiplierThreshold".into()
        }
        TagKind::PerStat(x) => {
            names(&mut m, "stat", "statList", &x.stats);
            opt_sym(&mut m, "actor", x.actor);
            opt_num(&mut m, "div", x.div);
            opt_sym(&mut m, "divVar", x.div_var);
            opt_num(&mut m, "limit", x.limit);
            opt_sym(&mut m, "limitVar", x.limit_var);
            yes(&mut m, "limitTotal", x.limit_total);
            opt_num(&mut m, "base", x.base);
            "PerStat".into()
        }
        TagKind::PercentStat(x) => {
            names(&mut m, "stat", "statList", &x.stats);
            opt_sym(&mut m, "actor", x.actor);
            opt_num(&mut m, "percent", x.percent);
            opt_sym(&mut m, "percentVar", x.percent_var);
            yes(&mut m, "floor", x.floor);
            opt_num(&mut m, "limit", x.limit);
            opt_sym(&mut m, "limitVar", x.limit_var);
            yes(&mut m, "limitTotal", x.limit_total);
            opt_num(&mut m, "base", x.base);
            "PercentStat".into()
        }
        TagKind::StatThreshold(x) => {
            names(&mut m, "stat", "statList", &x.stats);
            opt_num(&mut m, "threshold", x.threshold);
            opt_sym(&mut m, "thresholdStat", x.threshold_stat);
            opt_num(&mut m, "thresholdPercent", x.threshold_percent);
            opt_sym(&mut m, "thresholdPercentVar", x.threshold_percent_var);
            yes(&mut m, "upper", x.upper);
            "StatThreshold".into()
        }
        TagKind::DistanceRamp(r) => {
            m.insert(
                "ramp".into(),
                J::Array(r.iter().map(|(a, b)| nums(&[*a, *b])).collect()),
            );
            "DistanceRamp".into()
        }
        TagKind::MeleeProximity(r) => {
            m.insert("ramp".into(), nums(r));
            "MeleeProximity".into()
        }
        TagKind::Limit { limit, limit_var } => {
            opt_num(&mut m, "limit", *limit);
            opt_sym(&mut m, "limitVar", *limit_var);
            "Limit".into()
        }
        TagKind::Condition { vars, neg, actor } => {
            names(&mut m, "var", "varList", &Some(vars.clone()));
            yes(&mut m, "neg", *neg);
            opt_sym(&mut m, "actor", *actor);
            "Condition".into()
        }
        TagKind::ActorCondition { actor, vars, neg } => {
            opt_sym(&mut m, "actor", *actor);
            names(&mut m, "var", "varList", vars);
            yes(&mut m, "neg", *neg);
            "ActorCondition".into()
        }
        TagKind::ItemCondition(x) => {
            m.insert("itemSlot".into(), json!(&*x.item_slot));
            yes(&mut m, "allSlots", x.all_slots);
            yes(&mut m, "bothSlots", x.both_slots);
            yes(&mut m, "excludeSelf", x.exclude_self);
            opt_text(&mut m, "searchCond", &x.search_cond);
            opt_text(&mut m, "rarityCond", &x.rarity_cond);
            opt_text(&mut m, "nameCond", &x.name_cond);
            for (k, v) in [
                ("corruptedCond", x.corrupted_cond),
                ("shaperCond", x.shaper_cond),
                ("elderCond", x.elder_cond),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), J::Bool(v));
                }
            }
            yes(&mut m, "neg", x.neg);
            "ItemCondition".into()
        }
        TagKind::SocketedIn(x) => {
            opt_sym(&mut m, "slotName", x.slot_name);
            opt_text(&mut m, "keyword", &x.keyword);
            opt_text(&mut m, "socketColor", &x.socket_color);
            match &x.sockets {
                Some(Sockets::All) => {
                    m.insert("sockets".into(), json!("all"));
                }
                Some(Sockets::Numbers(n)) => {
                    m.insert("sockets".into(), nums(n));
                }
                Some(Sockets::Fewer(n)) => {
                    m.insert("sockets".into(), number(*n));
                }
                None => {}
            }
            yes(&mut m, "neg", x.neg);
            "SocketedIn".into()
        }
        TagKind::SkillName {
            names: n,
            include_transfigured,
            summon_skill,
            neg,
            skill_id,
        } => {
            names(&mut m, "skillName", "skillNameList", n);
            yes(&mut m, "includeTransfigured", *include_transfigured);
            yes(&mut m, "summonSkill", *summon_skill);
            yes(&mut m, "neg", *neg);
            opt_text(&mut m, "skillId", skill_id);
            "SkillName".into()
        }
        TagKind::SkillId(id) => {
            opt_text(&mut m, "skillId", id);
            "SkillId".into()
        }
        TagKind::SkillPart { list, part, neg } => {
            if let Some(l) = list {
                m.insert("skillPartList".into(), nums(l));
            }
            opt_num(&mut m, "skillPart", *part);
            yes(&mut m, "neg", *neg);
            "SkillPart".into()
        }
        TagKind::SkillType {
            list,
            skill_type,
            neg,
        } => {
            if let Some(l) = list {
                m.insert("skillTypeList".into(), nums(l));
            }
            opt_num(&mut m, "skillType", *skill_type);
            yes(&mut m, "neg", *neg);
            "SkillType".into()
        }
        TagKind::BaseFlag { flag, neg } => {
            m.insert("baseFlag".into(), json!(flag.as_str()));
            yes(&mut m, "neg", *neg);
            "BaseFlag".into()
        }
        TagKind::SlotName { names: n, neg } => {
            names(&mut m, "slotName", "slotNameList", n);
            yes(&mut m, "neg", *neg);
            "SlotName".into()
        }
        TagKind::ModFlagOr(f) => {
            m.insert("modFlags".into(), number(*f as f64));
            "ModFlagOr".into()
        }
        TagKind::KeywordFlagAnd(f) => {
            m.insert("keywordFlags".into(), number(*f as f64));
            "KeywordFlagAnd".into()
        }
        TagKind::MonsterTag { names: n, neg } => {
            names(&mut m, "monsterTag", "monsterTagList", n);
            yes(&mut m, "neg", *neg);
            "MonsterTag".into()
        }
        TagKind::Other { kind, fields } => {
            if let J::Object(f) = value(&ModValue::Record(fields.clone())) {
                m.extend(f);
            }
            kind.as_str().to_string()
        }
    };
    m.insert("type".into(), json!(kind));
    if let Some((limit, key)) = t.global_limit {
        m.insert("globalLimit".into(), number(limit));
        m.insert("globalLimitKey".into(), json!(key.as_str()));
    }
    J::Object(m)
}

pub fn mod_(m: &Mod) -> J {
    let mut o = Map::new();
    o.insert("name".into(), json!(m.name.as_str()));
    o.insert("type".into(), json!(m.typ.name()));
    o.insert("value".into(), value(&m.value));
    o.insert("flags".into(), number(m.flags as f64));
    o.insert("keywordFlags".into(), number(m.keyword_flags as f64));
    if let Some(s) = &m.source {
        o.insert("source".into(), json!(&**s));
    }
    if let Some(s) = &m.source_slot {
        o.insert("sourceSlot".into(), json!(&**s));
    }
    for (i, t) in m.tags.iter().enumerate() {
        o.insert((i + 1).to_string(), tag(t));
    }
    for (i, t) in &m.unreached_tags {
        o.insert(i.to_string(), tag(t));
    }
    J::Object(o)
}

/// The oracle's encoding normalised to what the typed form keeps: numbers
/// as floats, numeric strings in tags as numbers, `false` flags of tags
/// left out (they mean the same as absent ones).
pub fn normalise(v: &J, in_tag: bool) -> J {
    match v {
        J::Number(n) => n.as_f64().map_or(J::Null, number),
        J::String(s) if in_tag => match engine::mods::store::coerce(s) {
            Some(n) if !s.is_empty() => number(n),
            _ => v.clone(),
        },
        J::Array(a) => J::Array(a.iter().map(|x| normalise(x, in_tag)).collect()),
        J::Object(o) => {
            // A tag: a table with a type next to a mod's indices
            let is_tag = o.contains_key("type") && !o.contains_key("name");
            J::Object(
                o.iter()
                    .filter(|(k, x)| !(is_tag && **x == J::Bool(false) && !k.ends_with("Cond")))
                    .map(|(k, x)| {
                        let tag_field = is_tag && k != "type" && !k.ends_with("Cond");
                        // Upstream visits skillTypeList with pairs, so a
                        // sparse one is its values in order
                        if k == "skillTypeList"
                            && let J::Object(sparse) = x
                        {
                            let mut keyed: Vec<(i64, &J)> = sparse
                                .iter()
                                .filter_map(|(i, v)| Some((i.parse().ok()?, v)))
                                .collect();
                            keyed.sort_by_key(|(i, _)| *i);
                            let list = keyed.into_iter().map(|(_, v)| normalise(v, true)).collect();
                            return (k.clone(), J::Array(list));
                        }
                        (k.clone(), normalise(x, tag_field))
                    })
                    .collect(),
            )
        }
        v => v.clone(),
    }
}
