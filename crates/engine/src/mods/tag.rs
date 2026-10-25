//! Mod tags: the conditions and scaling `ModStore:EvalMod` applies
//! (`Classes/ModStore.lua`). Field names follow upstream's.

use super::sym::Sym;
use super::types::Record;
use std::rc::Rc;

/// A tag, and the global limit it may carry (`globalLimit`, `globalLimitKey`),
/// which caps the sum of all mods sharing the key within one query.
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub kind: TagKind,
    pub global_limit: Option<(f64, Sym)>,
}

/// `var` or `varList` (`stat` or `statList`, ...)
#[derive(Clone, Debug, PartialEq)]
pub enum Names {
    One(Sym),
    List(Vec<Sym>),
}

impl Names {
    pub fn iter(&self) -> impl Iterator<Item = Sym> + '_ {
        let (one, list) = match self {
            Names::One(s) => (Some(*s), &[][..]),
            Names::List(l) => (None, &l[..]),
        };
        one.into_iter().chain(list.iter().copied())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TagKind {
    Multiplier(Multiplier),
    MultiplierThreshold(MultiplierThreshold),
    PerStat(PerStat),
    PercentStat(PercentStat),
    StatThreshold(StatThreshold),
    /// Value scaled by skill distance, between `(distance, factor)` points
    DistanceRamp(Vec<(f64, f64)>),
    /// `ramp = { max, min }`; only the first is used
    MeleeProximity(Vec<f64>),
    Limit {
        limit: Option<f64>,
        limit_var: Option<Sym>,
    },
    Condition {
        vars: Names,
        neg: bool,
        /// Set by a few of upstream's mod lines, read by nothing
        actor: Option<Sym>,
    },
    ActorCondition {
        actor: Option<Sym>,
        vars: Option<Names>,
        neg: bool,
    },
    ItemCondition(Rc<ItemCondition>),
    SocketedIn(Rc<SocketedIn>),
    SkillName {
        names: Option<Names>,
        include_transfigured: bool,
        summon_skill: bool,
        neg: bool,
        /// Set on one tag by upstream's data, read by nothing
        skill_id: Option<Rc<str>>,
    },
    SkillId(Option<Rc<str>>),
    /// `skillPartList`, or `skillPart`, which may be nil
    SkillPart {
        list: Option<Vec<f64>>,
        part: Option<f64>,
        neg: bool,
    },
    /// `skillTypeList`, or `skillType`
    SkillType {
        list: Option<Vec<f64>>,
        skill_type: Option<f64>,
        neg: bool,
    },
    BaseFlag {
        flag: Sym,
        neg: bool,
    },
    SlotName {
        names: Option<Names>,
        neg: bool,
    },
    ModFlagOr(u32),
    KeywordFlagAnd(u32),
    MonsterTag {
        names: Option<Names>,
        neg: bool,
    },
    /// Tags `EvalMod` does not look at (`Global`, `GlobalEffect`,
    /// `SlotNumber`, ...), kept for the code that reads them
    Other {
        kind: Sym,
        fields: Record,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Multiplier {
    pub vars: Option<Names>,
    pub actor: Option<Sym>,
    pub limit_actor: Option<Sym>,
    pub div: Option<f64>,
    pub div_var: Option<Sym>,
    pub no_floor: bool,
    pub limit: Option<f64>,
    pub limit_var: Option<Sym>,
    pub limit_stat: Option<Sym>,
    pub limit_total: bool,
    pub limit_neg_total: bool,
    pub invert: bool,
    pub base: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiplierThreshold {
    pub vars: Option<Names>,
    pub actor: Option<Sym>,
    pub threshold_actor: Option<Sym>,
    pub threshold: Option<f64>,
    pub threshold_var: Option<Sym>,
    pub upper: bool,
    pub equals: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerStat {
    pub stats: Option<Names>,
    pub actor: Option<Sym>,
    pub div: Option<f64>,
    pub div_var: Option<Sym>,
    pub limit: Option<f64>,
    pub limit_var: Option<Sym>,
    pub limit_total: bool,
    pub base: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PercentStat {
    pub stats: Option<Names>,
    pub actor: Option<Sym>,
    pub percent: Option<f64>,
    pub percent_var: Option<Sym>,
    pub floor: bool,
    pub limit: Option<f64>,
    pub limit_var: Option<Sym>,
    pub limit_total: bool,
    pub base: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatThreshold {
    pub stats: Option<Names>,
    pub threshold: Option<f64>,
    pub threshold_stat: Option<Sym>,
    pub threshold_percent: Option<f64>,
    pub threshold_percent_var: Option<Sym>,
    pub upper: bool,
}

/// Conditions on equipped items, which need the item data to evaluate
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemCondition {
    pub item_slot: Rc<str>,
    pub all_slots: bool,
    pub both_slots: bool,
    pub exclude_self: bool,
    pub search_cond: Option<Rc<str>>,
    pub rarity_cond: Option<Rc<str>>,
    pub corrupted_cond: Option<bool>,
    pub shaper_cond: Option<bool>,
    pub elder_cond: Option<bool>,
    pub name_cond: Option<Rc<str>>,
    pub neg: bool,
}

/// `sockets`: `"all"`, socket numbers, or a gem count
#[derive(Clone, Debug, PartialEq)]
pub enum Sockets {
    All,
    Numbers(Vec<f64>),
    Fewer(f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SocketedIn {
    pub slot_name: Option<Sym>,
    pub keyword: Option<Rc<str>>,
    pub socket_color: Option<Rc<str>>,
    pub sockets: Option<Sockets>,
    pub neg: bool,
}

impl Tag {
    /// Rewrites the tag's string fields (not its type, nor lists) with `f`,
    /// as upstream's `for k, v in pairs(tag) do if type(v) == "string"`
    /// loops do; `f` gives `None` to keep a string.
    pub fn map_strings(&mut self, f: &dyn Fn(&str) -> Option<String>) {
        let sym = |s: &mut Sym| {
            if let Some(n) = f(s.as_str()) {
                *s = Sym::new(&n);
            }
        };
        let rc = |s: &mut Rc<str>| {
            if let Some(n) = f(s) {
                *s = n.into();
            }
        };
        fn opt<T>(o: &mut Option<T>, g: impl Fn(&mut T)) {
            if let Some(x) = o {
                g(x)
            }
        }
        let names = |n: &mut Names| {
            if let Names::One(s) = n {
                sym(s)
            }
        };
        if let Some((_, key)) = &mut self.global_limit {
            sym(key);
        }
        match &mut self.kind {
            TagKind::Multiplier(m) => {
                opt(&mut m.vars, names);
                opt(&mut m.actor, sym);
                opt(&mut m.limit_actor, sym);
                opt(&mut m.div_var, sym);
                opt(&mut m.limit_var, sym);
                opt(&mut m.limit_stat, sym);
            }
            TagKind::MultiplierThreshold(m) => {
                opt(&mut m.vars, names);
                opt(&mut m.actor, sym);
                opt(&mut m.threshold_actor, sym);
                opt(&mut m.threshold_var, sym);
            }
            TagKind::PerStat(m) => {
                opt(&mut m.stats, names);
                opt(&mut m.actor, sym);
                opt(&mut m.div_var, sym);
                opt(&mut m.limit_var, sym);
            }
            TagKind::PercentStat(m) => {
                opt(&mut m.stats, names);
                opt(&mut m.actor, sym);
                opt(&mut m.percent_var, sym);
                opt(&mut m.limit_var, sym);
            }
            TagKind::StatThreshold(m) => {
                opt(&mut m.stats, names);
                opt(&mut m.threshold_stat, sym);
                opt(&mut m.threshold_percent_var, sym);
            }
            TagKind::DistanceRamp(_) | TagKind::MeleeProximity(_) => {}
            TagKind::Limit { limit_var, .. } => opt(limit_var, sym),
            TagKind::Condition { vars, actor, .. } => {
                names(vars);
                opt(actor, sym);
            }
            TagKind::ActorCondition { actor, vars, .. } => {
                opt(actor, sym);
                opt(vars, names);
            }
            TagKind::ItemCondition(c) => {
                let c = Rc::make_mut(c);
                rc(&mut c.item_slot);
                opt(&mut c.search_cond, rc);
                opt(&mut c.rarity_cond, rc);
                opt(&mut c.name_cond, rc);
            }
            TagKind::SocketedIn(c) => {
                let c = Rc::make_mut(c);
                opt(&mut c.slot_name, sym);
                opt(&mut c.keyword, rc);
                opt(&mut c.socket_color, rc);
            }
            TagKind::SkillName {
                names: n, skill_id, ..
            } => {
                opt(n, names);
                opt(skill_id, rc);
            }
            TagKind::SkillId(id) => opt(id, rc),
            TagKind::SkillPart { .. } | TagKind::SkillType { .. } => {}
            TagKind::BaseFlag { flag, .. } => sym(flag),
            TagKind::SlotName { names: n, .. } => opt(n, names),
            TagKind::ModFlagOr(_) | TagKind::KeywordFlagAnd(_) => {}
            TagKind::MonsterTag { names: n, .. } => opt(n, names),
            TagKind::Other { fields, .. } => {
                for (_, v) in &mut fields.fields {
                    if let super::types::ModValue::Str(s) = v {
                        rc(s);
                    }
                }
            }
        }
    }
}
