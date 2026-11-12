//! The mod parser's tables in typed form, as `gamedata` makes them from
//! upstream's `ModParser.lua` and `Data/ModCache.lua` at build time, in the
//! binary form of [`crate::codec`].

use super::{InitError, Port, ports};
use crate::codec::{Codec, Reader, Result as CResult, Writer};
use crate::codec_struct;
use crate::mods::{Mod, ModValue, Tag};
use std::collections::HashMap;
use std::rc::Rc;

/// What `formList` gives: how a line's value combines
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Form {
    Inc,
    Red,
    More,
    Less,
    Base,
    Gain,
    Lose,
    Grants,
    GrantsGlobal,
    Removes,
    Chance,
    RegenPercent,
    RegenFlat,
    DegenPercent,
    DegenFlat,
    Degen,
    Dmg,
    DmgAttacks,
    DmgSpells,
    DmgBoth,
    Flag,
    Override,
    Doubled,
    Pen,
    BaseCost,
    TotalCost,
}

const FORMS: &[(&str, Form)] = &[
    ("INC", Form::Inc),
    ("RED", Form::Red),
    ("MORE", Form::More),
    ("LESS", Form::Less),
    ("BASE", Form::Base),
    ("GAIN", Form::Gain),
    ("LOSE", Form::Lose),
    ("GRANTS", Form::Grants),
    ("GRANTS_GLOBAL", Form::GrantsGlobal),
    ("REMOVES", Form::Removes),
    ("CHANCE", Form::Chance),
    ("REGENPERCENT", Form::RegenPercent),
    ("REGENFLAT", Form::RegenFlat),
    ("DEGENPERCENT", Form::DegenPercent),
    ("DEGENFLAT", Form::DegenFlat),
    ("DEGEN", Form::Degen),
    ("DMG", Form::Dmg),
    ("DMGATTACKS", Form::DmgAttacks),
    ("DMGSPELLS", Form::DmgSpells),
    ("DMGBOTH", Form::DmgBoth),
    ("FLAG", Form::Flag),
    ("OVERRIDE", Form::Override),
    ("DOUBLED", Form::Doubled),
    ("PEN", Form::Pen),
    ("BASECOST", Form::BaseCost),
    ("TOTALCOST", Form::TotalCost),
];

impl Form {
    pub fn new(name: &str) -> Option<Form> {
        FORMS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
    }

    pub fn name(self) -> &'static str {
        FORMS.iter().find(|(_, f)| *f == self).expect("form").0
    }
}

impl Codec for Form {
    fn put(&self, w: &mut Writer) {
        w.str(self.name());
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        let n = r.str()?;
        Form::new(&n).ok_or_else(|| format!("unknown mod form {n}"))
    }
}

/// A table parseMod combines into a line's mods: the name list
/// (`modNameList` and the resource types), and the flags, tags and
/// directions of the other lists. `table` is false for a plain name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frag {
    pub table: bool,
    pub names: Vec<Rc<str>>,
    pub flags: u32,
    pub keyword_flags: u32,
    pub tag: Option<Tag>,
    pub tag_list: Option<Vec<Tag>>,
    pub add_to_aura: Option<bool>,
    pub only_add_to_banners: Option<bool>,
    pub new_aura: Option<bool>,
    pub new_aura_only_allies: Option<bool>,
    pub add_to_minion: Option<bool>,
    pub add_to_minion_tag: Option<Tag>,
    pub player_tag: Option<Tag>,
    pub player_tag_list: Option<Vec<Tag>>,
    pub add_to_skill: Option<Tag>,
    pub apply_to_enemy: Option<bool>,
    pub actor_enemy: Option<bool>,
    pub mod_suffix: Option<Rc<str>>,
}

codec_struct!(Frag {
    table,
    names,
    flags,
    keyword_flags,
    tag,
    tag_list,
    add_to_aura,
    only_add_to_banners,
    new_aura,
    new_aura_only_allies,
    add_to_minion,
    add_to_minion_tag,
    player_tag,
    player_tag_list,
    add_to_skill,
    apply_to_enemy,
    actor_enemy,
    mod_suffix,
});

impl Frag {
    pub fn name(name: &str) -> Frag {
        Frag {
            names: vec![name.into()],
            ..Frag::default()
        }
    }

    pub fn names(names: Vec<Rc<str>>) -> Frag {
        Frag {
            table: true,
            names,
            ..Frag::default()
        }
    }

    /// `misc[k] = v` for each of this table's keys
    pub fn merge_into(&self, misc: &mut Frag) {
        fn set<T: Clone>(to: &mut Option<T>, from: &Option<T>) {
            if from.is_some() {
                to.clone_from(from);
            }
        }
        set(&mut misc.add_to_aura, &self.add_to_aura);
        set(&mut misc.only_add_to_banners, &self.only_add_to_banners);
        set(&mut misc.new_aura, &self.new_aura);
        set(&mut misc.new_aura_only_allies, &self.new_aura_only_allies);
        set(&mut misc.add_to_minion, &self.add_to_minion);
        set(&mut misc.add_to_minion_tag, &self.add_to_minion_tag);
        set(&mut misc.player_tag, &self.player_tag);
        set(&mut misc.player_tag_list, &self.player_tag_list);
        set(&mut misc.add_to_skill, &self.add_to_skill);
        set(&mut misc.apply_to_enemy, &self.apply_to_enemy);
        set(&mut misc.actor_enemy, &self.actor_enemy);
        set(&mut misc.mod_suffix, &self.mod_suffix);
    }
}

/// A `flagTypes` entry: a flag's name, or a whole mod
#[derive(Clone, Debug, PartialEq)]
pub enum FlagType {
    Name(Rc<str>),
    Mod {
        name: Rc<str>,
        typ: Rc<str>,
        value: ModValue,
    },
}

impl Codec for FlagType {
    fn put(&self, w: &mut Writer) {
        match self {
            FlagType::Name(n) => {
                0u8.put(w);
                n.put(w);
            }
            FlagType::Mod { name, typ, value } => {
                1u8.put(w);
                name.put(w);
                typ.put(w);
                value.put(w);
            }
        }
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        Ok(match u8::get(r)? {
            0 => FlagType::Name(Codec::get(r)?),
            1 => FlagType::Mod {
                name: Codec::get(r)?,
                typ: Codec::get(r)?,
                value: Codec::get(r)?,
            },
            k => return Err(format!("bad flag type {k}")),
        })
    }
}

/// A ported closure of upstream's tables, by its key
#[derive(Clone)]
pub struct Closure {
    pub key: Rc<str>,
    pub hash: u64,
    pub port: &'static Port,
}

impl std::fmt::Debug for Closure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Closure({})", self.key)
    }
}

impl PartialEq for Closure {
    fn eq(&self, o: &Closure) -> bool {
        self.key == o.key && self.hash == o.hash
    }
}

impl Closure {
    /// The port of the closure stored at `key`, made from Lua whose source
    /// hashes to `hash`
    pub fn new(key: &str, hash: u64) -> Result<Closure, InitError> {
        match ports().get(key) {
            Some(p) if p.hash == hash => Ok(Closure {
                key: key.into(),
                hash,
                port: p,
            }),
            Some(p) => Err(InitError::Changed {
                key: key.into(),
                expected: p.hash,
                found: hash,
            }),
            None => Err(InitError::Unported { key: key.into() }),
        }
    }
}

impl Codec for Closure {
    fn put(&self, w: &mut Writer) {
        self.key.put(w);
        self.hash.put(w);
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        let key: Rc<str> = Codec::get(r)?;
        let hash = u64::get(r)?;
        Closure::new(&key, hash).map_err(|e| e.to_string())
    }
}

/// The special mods the gem loop of ModParser.lua makes for each skill
/// that read the line's number
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GemKind {
    MineThrowingSpeed,
    TrapThrowingSpeed,
    Chains,
    FiresArrow,
    FiresArrows,
    Pierces,
    FiresProjectiles,
}

impl Codec for GemKind {
    fn put(&self, w: &mut Writer) {
        (*self as u8).put(w);
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        use GemKind::*;
        Ok(
            match [
                MineThrowingSpeed,
                TrapThrowingSpeed,
                Chains,
                FiresArrow,
                FiresArrows,
                Pierces,
                FiresProjectiles,
            ]
            .get(u8::get(r)? as usize)
            {
                Some(k) => *k,
                None => return Err("bad gem rule".into()),
            },
        )
    }
}

/// A pattern table's value: given, or computed from the line's captures
#[derive(Clone, Debug, PartialEq)]
pub enum Entry<T> {
    Value(T),
    Func(Closure),
    /// A gem loop rule, for the skill named
    Gem(GemKind, Rc<str>),
}

impl<T: Codec> Codec for Entry<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            Entry::Value(v) => {
                0u8.put(w);
                v.put(w);
            }
            Entry::Func(c) => {
                1u8.put(w);
                c.put(w);
            }
            Entry::Gem(k, s) => {
                2u8.put(w);
                k.put(w);
                s.put(w);
            }
        }
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        Ok(match u8::get(r)? {
            0 => Entry::Value(Codec::get(r)?),
            1 => Entry::Func(Codec::get(r)?),
            2 => Entry::Gem(Codec::get(r)?, Codec::get(r)?),
            k => return Err(format!("bad pattern entry {k}")),
        })
    }
}

/// A pattern table in source order
#[derive(Clone, Debug, PartialEq)]
pub struct List<T> {
    pub patterns: Vec<String>,
    pub entries: Vec<Entry<T>>,
}

impl<T> Default for List<T> {
    fn default() -> Self {
        List {
            patterns: vec![],
            entries: vec![],
        }
    }
}

impl<T: Codec> Codec for List<T> {
    fn put(&self, w: &mut Writer) {
        self.patterns.put(w);
        self.entries.put(w);
    }
    fn get(r: &mut Reader) -> CResult<Self> {
        let l = List {
            patterns: Codec::get(r)?,
            entries: Codec::get(r)?,
        };
        if l.patterns.len() != l.entries.len() {
            return Err("pattern table of mismatched length".into());
        }
        Ok(l)
    }
}

impl<T> List<T> {
    /// `t[pattern] = entry`, keeping the first position of a pattern
    pub fn set(&mut self, pattern: String, entry: Entry<T>) {
        match self.patterns.iter().position(|p| *p == pattern) {
            Some(i) => self.entries[i] = entry,
            None => {
                self.patterns.push(pattern);
                self.entries.push(entry);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// A `jewelFuncList` entry: `{ func = ..., type = ... }`
#[derive(Clone, Debug, PartialEq)]
pub struct JewelFunc {
    pub typ: Rc<str>,
    pub value: ModValue,
}

codec_struct!(JewelFunc { typ, value });

/// What `extraSupport` reads of a skill's support gem, found by the
/// skill's name: its granted effect and its second one
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupportGem {
    pub gem: Option<GemEffects>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GemEffects {
    pub granted: Option<ModValue>,
    pub secondary: Option<Secondary>,
}

/// A gem's second effect, and whether it is a support
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Secondary {
    pub id: Option<ModValue>,
    pub support: bool,
}

codec_struct!(SupportGem { gem });
codec_struct!(GemEffects { granted, secondary });
codec_struct!(Secondary { id, support });

/// A parse result: the mods (none if the line is not understood) and the
/// part of the line not understood
pub type Parsed = (Option<Vec<Mod>>, Option<String>);

/// The parser's tables
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub form_list: List<Form>,
    pub mod_name_list: List<Frag>,
    pub mod_flag_list: List<Frag>,
    pub pre_flag_list: List<Frag>,
    pub mod_tag_list: List<Frag>,
    pub special_mod_list: List<Vec<Mod>>,
    pub unsupported_mod_list: Vec<String>,
    pub suffix_types: List<Rc<str>>,
    pub dmg_types: HashMap<String, Rc<str>>,
    pub pen_types: List<Frag>,
    pub flag_types: List<FlagType>,
    pub regen_types: HashMap<String, Frag>,
    pub degen_types: HashMap<String, Frag>,
    pub cost_types: List<Frag>,
    pub base_cost_types: List<Frag>,
    pub skill_name_list: List<Frag>,
    pub pre_skill_name_list: List<Frag>,
    pub jewel_func_list: List<JewelFunc>,
    pub cluster_jewel_skills: HashMap<String, Vec<Mod>>,
    /// The module values closures read (`ModFlag`, `gemIdLookup`, the
    /// fields of `data` they use, ...), as records or lists
    pub upvalues: Vec<(String, ModValue)>,
    /// By skill id, for each id `gemIdLookup` gives
    pub support_gems: HashMap<String, SupportGem>,
}

codec_struct!(Rules {
    form_list,
    mod_name_list,
    mod_flag_list,
    pre_flag_list,
    mod_tag_list,
    special_mod_list,
    unsupported_mod_list,
    suffix_types,
    dmg_types,
    pen_types,
    flag_types,
    regen_types,
    degen_types,
    cost_types,
    base_cost_types,
    skill_name_list,
    pre_skill_name_list,
    jewel_func_list,
    cluster_jewel_skills,
    upvalues,
    support_gems,
});

/// The mod parser's typed data: its tables and upstream's mod cache
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParserData {
    pub rules: Rules,
    /// `Data/ModCache.lua`, line by line
    pub cache: HashMap<String, Parsed>,
}

codec_struct!(ParserData { rules, cache });

impl ParserData {
    pub fn decode(bytes: &[u8]) -> CResult<ParserData> {
        crate::codec::decode(bytes)
    }

    pub fn encode(&self) -> Vec<u8> {
        crate::codec::encode(self)
    }
}
