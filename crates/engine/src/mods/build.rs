//! Building mods and tags in code, as upstream's `modLib.createMod(...)`
//! with tag tables does: `Mod::new("Life", ModType::Base, 10.0, Some("Config"))
//! .tag(Tag::condition("Combat"))`.

use super::sym::Sym;
use super::tag::*;
use super::types::*;
use std::rc::Rc;

impl From<f64> for ModValue {
    fn from(n: f64) -> ModValue {
        ModValue::Number(n)
    }
}

impl From<bool> for ModValue {
    fn from(b: bool) -> ModValue {
        ModValue::Bool(b)
    }
}

impl From<&str> for ModValue {
    fn from(s: &str) -> ModValue {
        ModValue::Str(s.into())
    }
}

impl From<String> for ModValue {
    fn from(s: String) -> ModValue {
        ModValue::Str(s.into())
    }
}

impl From<Mod> for ModValue {
    fn from(m: Mod) -> ModValue {
        ModValue::Mod(Box::new(m))
    }
}

/// A record value with its fields in the given order, as a table
/// constructor gives them: `record([("key", "corpseLife".into()), ...])`
pub fn record<const N: usize>(fields: [(&str, ModValue); N]) -> ModValue {
    ModValue::Record(Record {
        fields: fields.into_iter().map(|(k, v)| (Sym::new(k), v)).collect(),
    })
}

impl Mod {
    /// `modLib.createMod(name, type, value, source)`: no flags, no tags
    pub fn new(name: &str, typ: ModType, value: impl Into<ModValue>, source: Option<&str>) -> Mod {
        Mod {
            name: Sym::new(name),
            typ,
            value: value.into(),
            flags: 0,
            keyword_flags: 0,
            source: source.map(Rc::from),
            source_slot: None,
            tags: vec![],
            unreached_tags: vec![],
        }
    }

    /// With mod flags and keyword flags
    pub fn flags(mut self, flags: u32, keyword_flags: u32) -> Mod {
        self.flags = flags;
        self.keyword_flags = keyword_flags;
        self
    }

    /// Adds a tag after the ones it has
    pub fn tag(mut self, tag: Tag) -> Mod {
        self.tags.push(tag);
        self
    }
}

impl Tag {
    fn kind(kind: TagKind) -> Tag {
        Tag {
            kind,
            global_limit: None,
        }
    }

    /// `{ type = "Condition", var = var }`
    pub fn condition(var: &str) -> Tag {
        Tag::kind(TagKind::Condition {
            vars: Names::One(Sym::new(var)),
            neg: false,
            actor: None,
        })
    }

    /// `{ type = "Condition", varList = vars }`
    pub fn condition_any(vars: &[&str]) -> Tag {
        Tag::kind(TagKind::Condition {
            vars: Names::List(vars.iter().map(|v| Sym::new(v)).collect()),
            neg: false,
            actor: None,
        })
    }

    /// `{ type = "ActorCondition", actor = actor, var = var }`
    pub fn actor_condition(actor: &str, var: &str) -> Tag {
        Tag::kind(TagKind::ActorCondition {
            actor: Some(Sym::new(actor)),
            vars: Some(Names::One(Sym::new(var))),
            neg: false,
        })
    }

    /// `{ type = "SkillName", skillName = name }`
    pub fn skill_name(name: &str) -> Tag {
        Tag::kind(TagKind::SkillName {
            names: Some(Names::One(Sym::new(name))),
            include_transfigured: false,
            summon_skill: false,
            neg: false,
            skill_id: None,
        })
    }

    /// `{ type = "SkillId", skillId = id }`
    pub fn skill_id(id: &str) -> Tag {
        Tag::kind(TagKind::SkillId(Some(id.into())))
    }

    /// `{ type = "SkillType", skillType = skill_type }`
    pub fn skill_type(skill_type: f64) -> Tag {
        Tag::kind(TagKind::SkillType {
            list: None,
            skill_type: Some(skill_type),
            neg: false,
        })
    }

    /// `{ type = "Multiplier", var = var }`
    pub fn multiplier(var: &str) -> Tag {
        Tag::kind(TagKind::Multiplier(Multiplier {
            vars: Some(Names::One(Sym::new(var))),
            ..Multiplier::default()
        }))
    }

    /// `{ type = "PerStat", stat = stat }`
    pub fn per_stat(stat: &str) -> Tag {
        Tag::kind(TagKind::PerStat(PerStat {
            stats: Some(Names::One(Sym::new(stat))),
            ..PerStat::default()
        }))
    }

    /// `{ type = "MultiplierThreshold", var = var, threshold = threshold }`
    pub fn multiplier_threshold(var: &str, threshold: f64) -> Tag {
        Tag::kind(TagKind::MultiplierThreshold(MultiplierThreshold {
            vars: Some(Names::One(Sym::new(var))),
            threshold: Some(threshold),
            ..MultiplierThreshold::default()
        }))
    }

    /// A tag the queries do not evaluate (`GlobalEffect`, `IgnoreCond`, ...)
    /// with its fields in order
    pub fn other<const N: usize>(kind: &str, fields: [(&str, ModValue); N]) -> Tag {
        Tag::kind(TagKind::Other {
            kind: Sym::new(kind),
            fields: Record {
                fields: fields.into_iter().map(|(k, v)| (Sym::new(k), v)).collect(),
            },
        })
    }

    /// `neg = true`
    pub fn negated(mut self) -> Tag {
        match &mut self.kind {
            TagKind::Condition { neg, .. }
            | TagKind::ActorCondition { neg, .. }
            | TagKind::SkillName { neg, .. }
            | TagKind::SkillPart { neg, .. }
            | TagKind::SkillType { neg, .. }
            | TagKind::BaseFlag { neg, .. }
            | TagKind::SlotName { neg, .. }
            | TagKind::MonsterTag { neg, .. } => *neg = true,
            k => panic!("{k:?} has no neg"),
        }
        self
    }

    /// `actor = actor`
    pub fn actor(mut self, a: &str) -> Tag {
        match &mut self.kind {
            TagKind::Multiplier(m) => m.actor = Some(Sym::new(a)),
            TagKind::MultiplierThreshold(m) => m.actor = Some(Sym::new(a)),
            TagKind::PerStat(m) => m.actor = Some(Sym::new(a)),
            TagKind::PercentStat(m) => m.actor = Some(Sym::new(a)),
            TagKind::Condition { actor, .. } | TagKind::ActorCondition { actor, .. } => {
                *actor = Some(Sym::new(a))
            }
            k => panic!("{k:?} has no actor"),
        }
        self
    }

    /// `base = base`
    pub fn base(mut self, base: f64) -> Tag {
        match &mut self.kind {
            TagKind::Multiplier(m) => m.base = Some(base),
            TagKind::PerStat(m) => m.base = Some(base),
            TagKind::PercentStat(m) => m.base = Some(base),
            k => panic!("{k:?} has no base"),
        }
        self
    }

    /// `div = div`
    pub fn divisor(mut self, div: f64) -> Tag {
        match &mut self.kind {
            TagKind::Multiplier(m) => m.div = Some(div),
            TagKind::PerStat(m) => m.div = Some(div),
            k => panic!("{k:?} has no div"),
        }
        self
    }

    /// `limit = limit`
    pub fn limit(mut self, limit: f64) -> Tag {
        match &mut self.kind {
            TagKind::Multiplier(m) => m.limit = Some(limit),
            TagKind::PerStat(m) => m.limit = Some(limit),
            TagKind::PercentStat(m) => m.limit = Some(limit),
            k => panic!("{k:?} has no limit"),
        }
        self
    }

    /// `limitStat = stat`
    pub fn limit_stat(mut self, stat: &str) -> Tag {
        match &mut self.kind {
            TagKind::Multiplier(m) => m.limit_stat = Some(Sym::new(stat)),
            k => panic!("{k:?} has no limitStat"),
        }
        self
    }

    /// `includeTransfigured = true`
    pub fn include_transfigured(mut self) -> Tag {
        match &mut self.kind {
            TagKind::SkillName {
                include_transfigured,
                ..
            } => *include_transfigured = true,
            k => panic!("{k:?} has no includeTransfigured"),
        }
        self
    }

    /// `summonSkill = true`
    pub fn summon_skill(mut self) -> Tag {
        match &mut self.kind {
            TagKind::SkillName { summon_skill, .. } => *summon_skill = true,
            k => panic!("{k:?} has no summonSkill"),
        }
        self
    }
}
