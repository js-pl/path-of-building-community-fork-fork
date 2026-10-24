//! Modifiers: what `modLib.createMod` builds in upstream (`Modules/ModTools.lua`).

use super::sym::Sym;
use super::tag::Tag;
use std::rc::Rc;

/// How a mod combines with others of its name.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModType {
    Base,
    Inc,
    More,
    Flag,
    Override,
    List,
    Max,
    Min,
    /// Types upstream uses rarely or by mistake (`"Flag"`, `"CHANCE"`, `"DUMMY"`),
    /// which only queries for that exact type find
    Other(Sym),
}

impl ModType {
    pub fn new(name: &str) -> ModType {
        match name {
            "BASE" => ModType::Base,
            "INC" => ModType::Inc,
            "MORE" => ModType::More,
            "FLAG" => ModType::Flag,
            "OVERRIDE" => ModType::Override,
            "LIST" => ModType::List,
            "MAX" => ModType::Max,
            "MIN" => ModType::Min,
            other => ModType::Other(Sym::new(other)),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ModType::Base => "BASE",
            ModType::Inc => "INC",
            ModType::More => "MORE",
            ModType::Flag => "FLAG",
            ModType::Override => "OVERRIDE",
            ModType::List => "LIST",
            ModType::Max => "MAX",
            ModType::Min => "MIN",
            ModType::Other(s) => s.as_str(),
        }
    }
}

/// A mod's value, or a value inside one.
#[derive(Clone, Debug, PartialEq)]
pub enum ModValue {
    Bool(bool),
    Number(f64),
    Str(Rc<str>),
    /// A mod carried by another, e.g. `MinionModifier`'s `{ mod = ... }`
    Mod(Box<Mod>),
    /// A structured value, e.g. `ExtraSkill`'s `{ skillId = ..., level = ... }`
    Record(Record),
    List(Vec<ModValue>),
    /// A function of upstream's data, by name, e.g. a radius jewel's
    Func(Rc<str>),
}

impl ModValue {
    /// Lua truthiness: everything but `false`.
    pub fn truthy(&self) -> bool {
        !matches!(self, ModValue::Bool(false))
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            ModValue::Number(n) => Some(*n),
            _ => None,
        }
    }
}

/// Named fields, in the order upstream's table constructor gives them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub fields: Vec<(Sym, ModValue)>,
}

impl Record {
    pub fn get(&self, name: &str) -> Option<&ModValue> {
        let name = Sym::new(name);
        self.fields.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, name: Sym) -> Option<&mut ModValue> {
        self.fields
            .iter_mut()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }
}

/// A modifier.
#[derive(Clone, Debug, PartialEq)]
pub struct Mod {
    pub name: Sym,
    pub typ: ModType,
    pub value: ModValue,
    pub flags: u32,
    pub keyword_flags: u32,
    pub source: Option<Rc<str>>,
    /// The item slot a mod from an item comes from (`sourceSlot`)
    pub source_slot: Option<Rc<str>>,
    /// Conditions and scaling, applied in order by `EvalMod`
    pub tags: Vec<Tag>,
}

impl Mod {
    /// `source:match("[^:]+")`: the source up to its first `:`, as queries
    /// filter by it.
    pub fn source_name(&self) -> Option<&str> {
        let s = self.source.as_deref()?.trim_start_matches(':');
        let end = s.find(':').unwrap_or(s.len());
        (end > 0).then(|| &s[..end])
    }
}
