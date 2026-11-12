//! Port of upstream `src/Modules/ModParser.lua`.
//!
//! The pattern tables come typed from `gamedata` ([`rules`]), made at build
//! time from upstream's source, with upstream's `Data/ModCache.lua`. Their
//! closures are ported to Rust: most are translated by `cargo run -p
//! importer --bin portgen` into `generated.rs`, the rest by hand in
//! `ported.rs`, and the tables address them by key. Each port records the
//! hash of the Lua source it was made from, and the tables are refused if
//! upstream's source changed, so a changed closure cannot go unnoticed.
//! Closures compute with [`val::V`] and give typed mods and fragments; the
//! parse itself is typed.

mod ctx;
mod generated;
mod ported;
pub mod rules;
pub mod val;

pub use ctx::{Ctx, make_mod};
pub use generated::DATA_FIELDS;
pub use rules::{Entry, Form, Frag, GemKind, List, Parsed, ParserData, Rules};

use crate::mods::convert::{Converter, DKey, Dyn, Kind};
use crate::mods::tag::TagKind;
use crate::mods::{Mod, ModType, ModValue, Record, Sym, Tag};
use lua::pattern;
use rules::FlagType;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use val::V;

/// A Rust port of one upstream closure.
pub struct Port {
    /// Where the closure is stored, as the importer names it
    pub key: &'static str,
    /// FNV-1a of the closure's Lua source
    pub hash: u64,
    pub f: fn(&Ctx, &[V]) -> Vec<V>,
}

/// Every port, by key
pub fn ports() -> &'static HashMap<&'static str, &'static Port> {
    static PORTS: OnceLock<HashMap<&'static str, &'static Port>> = OnceLock::new();
    PORTS.get_or_init(|| {
        generated::PORTS
            .iter()
            .chain(ported::PORTS)
            .map(|p| (p.key, p))
            .collect()
    })
}

/// Error in making the parser's tables
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
    /// A table holds what the typed form has no place for
    Table(String),
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
            InitError::Table(e) => write!(f, "ModParser table: {e}"),
        }
    }
}

impl std::error::Error for InitError {}

/// The pattern tables a scan marks as hit, for rule coverage
const SCANNED: [&str; 10] = [
    "formList",
    "modNameList",
    "modFlagList",
    "preFlagList",
    "modTagList",
    "specialModList",
    "suffixTypes",
    "flagTypes",
    "skillNameList",
    "preSkillNameList",
];

/// Coverage slot of a scan that is not reported
const UNREPORTED: usize = SCANNED.len();

pub struct ModParser {
    rules: Rules,
    ctx: Ctx,
    jewel_funcs: HashMap<String, usize>,
    unsupported: HashSet<String>,
    preloaded: HashMap<String, Parsed>,
    cache: RwLock<HashMap<Vec<u8>, Parsed>>,
    hits: Vec<Vec<AtomicBool>>,
    /// `modName[2]` that DOUBLED lines wrote into `modNameList` entries
    doubled: RwLock<HashMap<usize, Rc<str>>>,
}

/// Mods of an intermediate parse: a mod upstream would build with a nil
/// value or name only fails when the parse is the one returned
type Mods = Result<Vec<Mod>, String>;

/// A frag (or a plain name) from a closure's value
pub fn frag<D: Dyn>(v: &D) -> Result<Frag, String> {
    let conv = Converter::default();
    match v.kind() {
        Kind::Str(s) => return Ok(Frag::name(&s)),
        Kind::Table => {}
        _ => return Err("a pattern value that is neither a name nor a table".into()),
    }
    let mut f = Frag {
        table: true,
        ..Frag::default()
    };
    let bool_of = |x: &D, k: &str| -> Result<Option<bool>, String> {
        match x.kind() {
            Kind::Bool(b) => Ok(Some(b)),
            _ => Err(format!("{k} is not a boolean")),
        }
    };
    let tag_list = |x: &D| -> Result<Vec<Tag>, String> {
        crate::mods::convert::d_array(x)
            .iter()
            .map(|t| conv.tag(t))
            .collect()
    };
    let text = |x: &D| -> Result<Rc<str>, String> {
        match x.kind() {
            Kind::Str(s) => Ok(s.into()),
            _ => Err("a name is not a string".into()),
        }
    };
    let flag_bits = |x: &D| -> Result<u32, String> {
        match x.kind() {
            Kind::Num(n) => Ok(n.rem_euclid(4294967296.0) as u64 as u32),
            _ => Err("flags are not a number".into()),
        }
    };
    for (k, x) in v.entries() {
        let k = match k {
            DKey::Str(k) => k,
            DKey::Int(_) => continue,
            DKey::Other(k) => return Err(format!("pattern value key {k}")),
        };
        match k.as_str() {
            "flags" => f.flags = flag_bits(&x)?,
            "keywordFlags" => f.keyword_flags = flag_bits(&x)?,
            "tag" => f.tag = Some(conv.tag(&x)?),
            "tagList" => f.tag_list = Some(tag_list(&x)?),
            "addToAura" => f.add_to_aura = bool_of(&x, &k)?,
            "onlyAddToBanners" => f.only_add_to_banners = bool_of(&x, &k)?,
            "newAura" => f.new_aura = bool_of(&x, &k)?,
            "newAuraOnlyAllies" => f.new_aura_only_allies = bool_of(&x, &k)?,
            "addToMinion" => f.add_to_minion = bool_of(&x, &k)?,
            "applyToEnemy" => f.apply_to_enemy = bool_of(&x, &k)?,
            "actorEnemy" => f.actor_enemy = bool_of(&x, &k)?,
            "addToMinionTag" => f.add_to_minion_tag = Some(conv.tag(&x)?),
            "playerTag" => f.player_tag = Some(conv.tag(&x)?),
            "addToSkill" => f.add_to_skill = Some(conv.tag(&x)?),
            "playerTagList" => f.player_tag_list = Some(tag_list(&x)?),
            "modSuffix" => f.mod_suffix = Some(text(&x)?),
            k => return Err(format!("pattern value field {k}")),
        }
    }
    for x in crate::mods::convert::d_array(v) {
        f.names.push(text(&x)?);
    }
    if f.names.len()
        != v.entries()
            .iter()
            .filter(|(k, _)| matches!(k, DKey::Int(_)))
            .count()
    {
        return Err("a pattern value with holes in its names".into());
    }
    Ok(f)
}

/// Mods from a closure's value: a list of `mod(...)`s, or nil
fn mods_of(v: &V) -> Result<Option<Vec<Mod>>, String> {
    match v {
        V::Nil => Ok(None),
        V::Tab(_) => {
            let conv = Converter::default();
            let arr = crate::mods::convert::d_array(v);
            if arr.len() != v.len_pairs() {
                return Err("special mod list with other keys".into());
            }
            arr.iter()
                .map(|m| conv.mod_(m))
                .collect::<Result<_, _>>()
                .map(Some)
        }
        v => Err(format!("special mod list is a {} value", v.type_name())),
    }
}

impl Dyn for V {
    fn kind(&self) -> Kind<'_, V> {
        match self {
            V::Nil => Kind::Nil,
            V::Bool(b) => Kind::Bool(*b),
            V::Num(n) => Kind::Num(*n),
            V::Str(s) => Kind::Str(String::from_utf8_lossy(s)),
            V::Tab(_) => Kind::Table,
            V::Mod(m) => Kind::Typed(ModValue::Mod(Box::new((**m).clone()))),
            V::Func(f) => Kind::Typed(ModValue::Func(f.clone())),
        }
    }

    fn get(&self, k: &str) -> V {
        self.as_tab().map_or(V::Nil, |t| t.get_str(k).clone())
    }

    fn get_i(&self, i: i64) -> V {
        self.as_tab().map_or(V::Nil, |t| t.get_int(i).clone())
    }

    fn border(&self) -> usize {
        self.as_tab().map_or(0, |t| t.len())
    }

    fn len_pairs(&self) -> usize {
        self.as_tab().map_or(0, |t| t.len_pairs())
    }

    fn entries(&self) -> Vec<(DKey, V)> {
        self.as_tab().map_or(vec![], |t| {
            t.entries()
                .map(|(k, v)| {
                    let k = match k {
                        val::K::Str(s) => DKey::Str(String::from_utf8_lossy(&s).into_owned()),
                        val::K::Int(i) => DKey::Int(i),
                    };
                    (k, v.clone())
                })
                .collect()
        })
    }
}

/// A scan's result: the entry's position, its captures and the rest of
/// the line
struct Found {
    index: usize,
    caps: Vec<V>,
}

/// Tags parseMod adds for the form: `modExtraTags`
enum ExtraTags {
    None,
    Frag(Box<Frag>),
    /// Tags by mod, for DOUBLED's two mods
    PerMod(Vec<Vec<Tag>>),
}

fn hand_attack_tag() -> Frag {
    Frag {
        table: true,
        tag: Some(Tag {
            kind: TagKind::Condition {
                vars: crate::mods::tag::Names::One(Sym::new("{Hand}Attack")),
                neg: false,
                actor: None,
            },
            global_limit: None,
        }),
        ..Frag::default()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn wrap(name: &str, inner: Mod, extra: Vec<(&str, ModValue)>, tags: Vec<Tag>) -> Mod {
    let mut fields = vec![(Sym::new("mod"), ModValue::Mod(Box::new(inner)))];
    fields.extend(extra.into_iter().map(|(k, v)| (Sym::new(k), v)));
    Mod {
        name: Sym::new(name),
        typ: ModType::List,
        value: ModValue::Record(Record { fields }),
        flags: 0,
        keyword_flags: 0,
        source: None,
        source_slot: None,
        tags: tags.into(),
        unreached_tags: vec![],
    }
}

/// `actor = "enemy"` on a tag
fn set_actor(tag: &mut Tag, actor: &str) {
    let a = Some(Sym::new(actor));
    match &mut tag.kind {
        TagKind::Multiplier(x) => x.actor = a,
        TagKind::MultiplierThreshold(x) => x.actor = a,
        TagKind::PerStat(x) => x.actor = a,
        TagKind::PercentStat(x) => x.actor = a,
        TagKind::Condition { actor, .. } | TagKind::ActorCondition { actor, .. } => *actor = a,
        k => panic!("ModParser: setting the actor of a {k:?} tag"),
    }
}

impl ModParser {
    /// The parser on its typed tables.
    pub fn new(data: ParserData) -> ModParser {
        let ParserData { rules, cache } = data;
        let jewel_funcs = rules
            .jewel_func_list
            .patterns
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        let unsupported = rules.unsupported_mod_list.iter().cloned().collect();
        let lists: [usize; 10] = [
            rules.form_list.len(),
            rules.mod_name_list.len(),
            rules.mod_flag_list.len(),
            rules.pre_flag_list.len(),
            rules.mod_tag_list.len(),
            rules.special_mod_list.len(),
            rules.suffix_types.len(),
            rules.flag_types.len(),
            rules.skill_name_list.len(),
            rules.pre_skill_name_list.len(),
        ];
        let mut hits: Vec<Vec<AtomicBool>> = lists
            .iter()
            .map(|&n| (0..n).map(|_| AtomicBool::new(false)).collect())
            .collect();
        hits.push(
            (0..rules.pen_types.len().max(rules.cost_types.len()))
                .map(|_| AtomicBool::new(false))
                .collect(),
        );
        ModParser {
            ctx: Ctx::new(&rules),
            rules,
            jewel_funcs,
            unsupported,
            preloaded: cache,
            cache: RwLock::new(HashMap::new()),
            hits,
            doubled: RwLock::new(HashMap::new()),
        }
    }

    /// The parser from its binary form
    pub fn decode(bytes: &[u8]) -> crate::codec::Result<ModParser> {
        Ok(ModParser::new(ParserData::decode(bytes)?))
    }

    /// Patterns no scan has selected so far, by table: rule coverage of the
    /// lines parsed with this parser.
    pub fn unhit_rules(&self) -> Vec<(&'static str, String)> {
        let r = &self.rules;
        let patterns: [&Vec<String>; 10] = [
            &r.form_list.patterns,
            &r.mod_name_list.patterns,
            &r.mod_flag_list.patterns,
            &r.pre_flag_list.patterns,
            &r.mod_tag_list.patterns,
            &r.special_mod_list.patterns,
            &r.suffix_types.patterns,
            &r.flag_types.patterns,
            &r.skill_name_list.patterns,
            &r.pre_skill_name_list.patterns,
        ];
        let mut out = Vec::new();
        for (i, (name, list)) in SCANNED.iter().zip(patterns).enumerate() {
            for (j, p) in list.iter().enumerate() {
                if !self.hits[i][j].load(Ordering::Relaxed) {
                    out.push((*name, p.clone()));
                }
            }
        }
        out
    }

    /// `modLib.parseMod(line)`: the mods (none when the line is not
    /// understood) and the part of the line that was not understood.
    /// Results are cached, as upstream does, starting from its ModCache.
    /// Panics where upstream raises an error.
    pub fn parse(&self, line: &[u8]) -> Parsed {
        if let Some(r) = self.cache.read().expect("cache").get(line) {
            return r.clone();
        }
        if let Some(r) = std::str::from_utf8(line)
            .ok()
            .and_then(|l| self.preloaded.get(l))
        {
            return r.clone();
        }
        let (mut mods, mut extra) = self.parse_mod(line, 1);
        if mods.is_some() && extra.is_some() {
            (mods, extra) = self.parse_mod(line, 2);
        }
        let mods = mods.map(|m| m.unwrap_or_else(|e| panic!("ModParser: {e}")));
        let result = (mods, extra);
        self.cache
            .write()
            .expect("cache")
            .insert(line.to_vec(), result.clone());
        result
    }

    /// `scan(line, patternList, plain)`: the earliest, then longest match
    fn scan<T>(
        &self,
        line: &[u8],
        list: &List<T>,
        plain: bool,
        id: usize,
    ) -> (Option<Found>, Vec<u8>) {
        let lower = line.to_ascii_lowercase();
        let mut best: Option<(usize, usize, usize, usize, Vec<pattern::Capture>)> = None;
        for (i, p) in list.patterns.iter().enumerate() {
            let m = pattern::find(&lower, p.as_bytes(), 0, plain)
                .unwrap_or_else(|e| panic!("bad pattern {p:?}: {e:?}"));
            if let Some(m) = m {
                let better = match &best {
                    None => true,
                    Some((bs, be, bl, _, _)) => {
                        m.start < *bs
                            || (m.start == *bs && (m.end > *be || (m.end == *be && p.len() > *bl)))
                    }
                };
                if better {
                    best = Some((m.start, m.end, p.len(), i, m.captures));
                }
            }
        }
        let Some((start, end, _, index, mut caps)) = best else {
            return (None, line.to_vec());
        };
        if let Some(h) = self.hits[id].get(index) {
            h.store(true, Ordering::Relaxed);
        }
        caps.truncate(5);
        let caps = caps
            .iter()
            .map(|c| match *c {
                pattern::Capture::Str(a, b) => val::sb(&lower[a..b]),
                pattern::Capture::Pos(p) => val::n((p + 1) as f64),
            })
            .collect();
        let mut rest = line[..start].to_vec();
        rest.extend_from_slice(&line[end.min(line.len())..]);
        (Some(Found { index, caps }), rest)
    }

    fn call(&self, c: &rules::Closure, args: &[V]) -> V {
        val::first((c.port.f)(&self.ctx, args))
    }

    /// A scanned table's frag: its value, or its closure's on `args`
    fn frag_of(&self, list: &List<Frag>, found: &Found, args: impl FnOnce(&[V]) -> Vec<V>) -> Frag {
        match &list.entries[found.index] {
            Entry::Value(f) => f.clone(),
            Entry::Func(c) => {
                let v = self.call(c, &args(&found.caps));
                if v.is_nil() {
                    return Frag::default();
                }
                frag(&v).unwrap_or_else(|e| panic!("ModParser: {e}"))
            }
            Entry::Gem(..) => panic!("gem rule outside the special mods"),
        }
    }

    fn value_of<'a, T>(list: &'a List<T>, found: &Option<Found>) -> Option<&'a T> {
        match found.as_ref().map(|f| &list.entries[f.index]) {
            Some(Entry::Value(v)) => Some(v),
            Some(_) => panic!("unexpected function entry"),
            None => None,
        }
    }

    /// A gem loop rule's mods, with the line's number
    fn gem_mods(&self, kind: GemKind, skill: &str, num: V) -> Vec<Mod> {
        let tag = || {
            val::tbl(
                vec![
                    (val::s("type"), val::s("SkillName")),
                    (val::s("skillName"), val::s(skill)),
                    (val::s("includeTransfigured"), V::Bool(true)),
                ],
                vec![],
            )
        };
        let esm = |name: &str, args: Vec<V>| {
            let mut a = vec![val::s(name), val::s("BASE"), num.clone()];
            a.extend(args);
            let inner = make_mod(&a);
            make_mod(&[
                val::s("ExtraSkillMod"),
                val::s("LIST"),
                val::tbl(vec![(val::s("mod"), V::Mod(Rc::new(inner)))], vec![]),
                tag(),
            ])
        };
        let arrow = || self.ctx.field("KeywordFlag", "Arrow");
        let m = match kind {
            GemKind::MineThrowingSpeed => {
                let inner = make_mod(&[val::s("MineLayingSpeed"), val::s("INC"), num]);
                return vec![self.extra_skill_mod(inner, tag())];
            }
            GemKind::TrapThrowingSpeed => {
                let inner = make_mod(&[val::s("TrapThrowingSpeed"), val::s("INC"), num]);
                return vec![self.extra_skill_mod(inner, tag())];
            }
            GemKind::Chains => esm("ChainCountMax", vec![]),
            GemKind::FiresArrow => {
                let inner = make_mod(&[
                    val::s("ProjectileCount"),
                    val::s("BASE"),
                    val::n(1.0),
                    V::Nil,
                    val::n(0.0),
                    arrow(),
                ]);
                return vec![self.extra_skill_mod(inner, tag())];
            }
            GemKind::FiresArrows => esm("ProjectileCount", vec![V::Nil, val::n(0.0), arrow()]),
            GemKind::Pierces => make_mod(&[val::s("PierceCount"), val::s("BASE"), num, tag()]),
            GemKind::FiresProjectiles => esm("ProjectileCount", vec![]),
        };
        vec![m]
    }

    fn extra_skill_mod(&self, inner: Mod, tag: V) -> Mod {
        make_mod(&[
            val::s("ExtraSkillMod"),
            val::s("LIST"),
            val::tbl(vec![(val::s("mod"), V::Mod(Rc::new(inner)))], vec![]),
            tag,
        ])
    }

    /// A `modNameList` entry, with what DOUBLED lines wrote into it
    fn mod_name(&self, found: &Option<Found>) -> Option<(Frag, usize)> {
        let f = found.as_ref()?;
        let mut frag = Self::value_of(&self.rules.mod_name_list, found)?.clone();
        if let Some(second) = self.doubled.read().expect("doubled").get(&f.index) {
            match frag.names.len() {
                0 | 1 => frag.names.push(second.clone()),
                _ => frag.names[1] = second.clone(),
            }
        }
        Some((frag, f.index))
    }

    /// `parseMod(line, order)`
    fn parse_mod(&self, line_in: &[u8], order: u8) -> (Option<Mods>, Option<String>) {
        let r = &self.rules;
        let lower = line_in.to_ascii_lowercase();
        let one = |m: Mod| (Some(Ok(vec![m])), None);
        let jewel_mod = |v: ModValue| Mod {
            name: Sym::new("JewelFunc"),
            typ: ModType::List,
            value: v,
            flags: 0,
            keyword_flags: 0,
            source: None,
            source_slot: None,
            tags: Default::default(),
            unreached_tags: vec![],
        };

        // Jewel functions with captures
        for (pattern, entry) in r
            .jewel_func_list
            .patterns
            .iter()
            .zip(&r.jewel_func_list.entries)
        {
            let m = pattern::find(&lower, pattern.as_bytes(), 0, false).expect("jewel pattern");
            let Some(m) = m else { continue };
            if m.captures.is_empty() {
                continue;
            }
            let Entry::Value(jf) = entry else {
                unreachable!()
            };
            let mut args = vec![Some(ModValue::Str(pattern.as_str().into()))];
            args.extend(m.captures.iter().map(|c| {
                Some(match *c {
                    pattern::Capture::Str(a, b) => ModValue::Str(text(&lower[a..b]).into()),
                    pattern::Capture::Pos(p) => ModValue::Number((p + 1) as f64),
                })
            }));
            let func = ModValue::Func(Rc::new(crate::mods::Func {
                name: "jewelFunc".into(),
                args,
            }));
            let v = Record {
                fields: vec![
                    (Sym::new("func"), func),
                    (Sym::new("type"), ModValue::Str(jf.typ.clone())),
                ],
            };
            return one(jewel_mod(ModValue::Record(v)));
        }
        let lower_s = text(&lower);
        if let Some(&i) = self.jewel_funcs.get(&lower_s)
            && let Entry::Value(jf) = &r.jewel_func_list.entries[i]
        {
            return one(jewel_mod(jf.value.clone()));
        }
        if let Some(mods) = r.cluster_jewel_skills.get(&lower_s) {
            return (Some(Ok(mods.clone())), None);
        }
        if self.unsupported.contains(&lower_s) {
            return (Some(Ok(vec![])), Some(text(line_in)));
        }
        let (special, special_line) = self.scan(line_in, &r.special_mod_list, false, 5);
        if let Some(found) = special
            && special_line.is_empty()
        {
            let num = val::tonumber(&val::nth(&found.caps, 0));
            return match &r.special_mod_list.entries[found.index] {
                Entry::Func(c) => {
                    let mut args = vec![num];
                    args.extend(found.caps.iter().cloned());
                    let out = (c.port.f)(&self.ctx, &args);
                    let mods = mods_of(&val::nth(&out, 0)).map_err(|e| e.to_string());
                    let extra = match val::nth(&out, 1) {
                        V::Nil => None,
                        V::Str(s) => Some(text(&s)),
                        v => panic!("ModParser: special mod rest is a {} value", v.type_name()),
                    };
                    match mods {
                        Ok(m) => (m.map(Ok), extra),
                        Err(e) => (Some(Err(e)), extra),
                    }
                }
                Entry::Value(mods) => (Some(Ok(mods.clone())), None),
                Entry::Gem(kind, skill) => (Some(Ok(self.gem_mods(*kind, skill, num))), None),
            };
        }

        // Add-to-cluster-jewel special
        if let Some(m) =
            pattern::find_match(line_in, b"^Added Small Passive Skills also grant: (.+)$", 0)
                .expect("pattern")
            && let Some(pattern::Capture::Str(a, b)) = m.captures.first()
        {
            return one(make_mod(&[
                val::s("AddToClusterJewelNode"),
                val::s("LIST"),
                val::sb(&line_in[*a..*b]),
            ]));
        }

        let mut line = line_in.to_vec();
        line.push(b' ');

        // Flag/tag specification at the start of the line
        let (found, rest) = self.scan(&line, &r.pre_flag_list, false, 3);
        line = rest;
        let pre_flag = found.map(|f| self.frag_of(&r.pre_flag_list, &f, <[V]>::to_vec));

        // Skill name at the start of the line
        let (found, rest) = self.scan(&line, &r.pre_skill_name_list, false, 9);
        line = rest;
        let mut skill_tag = Self::value_of(&r.pre_skill_name_list, &found).cloned();

        // Modifier form
        let (found, rest) = self.scan(&line, &r.form_list, false, 0);
        line = rest;
        let Some(form_found) = found else {
            return (None, Some(text(&line)));
        };
        let Entry::Value(form) = r.form_list.entries[form_found.index] else {
            panic!("unexpected function entry");
        };
        let mut form_cap = form_found.caps;

        // Tags (per-charge, conditionals)
        let scan_tag = |line: &[u8]| -> (Option<Frag>, Vec<u8>) {
            let (found, rest) = self.scan(line, &r.mod_tag_list, false, 4);
            let tag = found.map(|f| {
                self.frag_of(&r.mod_tag_list, &f, |caps| {
                    let c1 = val::nth(caps, 0);
                    let first_arg =
                        if val::first(val::str_match(&c1, &val::s("%d+"), &V::Nil)).truthy() {
                            val::tonumber(&c1)
                        } else {
                            c1
                        };
                    let mut args = vec![first_arg];
                    args.extend(caps.iter().cloned());
                    args
                })
            });
            (tag, rest)
        };
        let (mod_tag, rest) = scan_tag(&line);
        line = rest;
        let mut mod_tag2 = None;
        if mod_tag
            .as_ref()
            .is_some_and(|t| t.table || !t.names.is_empty())
        {
            let (t2, rest) = scan_tag(&line);
            mod_tag2 = t2;
            line = rest;
        }

        // Modifier name and skill name
        if order == 2 && skill_tag.is_none() {
            let (found, rest) = self.scan(&line, &r.skill_name_list, false, 8);
            skill_tag = Self::value_of(&r.skill_name_list, &found).cloned();
            line = rest;
        }
        let mut mod_name: Option<(Frag, Option<usize>)>;
        match form {
            Form::Pen | Form::BaseCost | Form::TotalCost => {
                let list = match form {
                    Form::Pen => &r.pen_types,
                    Form::BaseCost => &r.base_cost_types,
                    _ => &r.cost_types,
                };
                let (found, rest) = self.scan(&line, list, true, UNREPORTED);
                line = rest;
                mod_name = Self::value_of(list, &found).cloned().map(|f| (f, None));
                if mod_name.is_none() {
                    return (Some(Ok(vec![])), Some(text(&line)));
                }
                line = self.scan(&line, &r.mod_name_list, true, 1).1;
            }
            Form::Flag => {
                let (found, rest) = self.scan(&line, &r.flag_types, false, 7);
                line = rest;
                let Some(flag) = Self::value_of(&r.flag_types, &found) else {
                    return (None, Some(text(&line)));
                };
                if form_cap.is_empty() {
                    form_cap.push(V::Nil);
                }
                form_cap[0] = match flag {
                    FlagType::Name(n) => val::s(n),
                    FlagType::Mod { name, typ, value } => val::tbl(
                        vec![
                            (val::s("name"), val::s(name)),
                            (val::s("type"), val::s(typ)),
                            (val::s("value"), V::from(value.clone())),
                        ],
                        vec![],
                    ),
                };
                let (found, rest) = self.scan(&line, &r.mod_name_list, true, 1);
                line = rest;
                mod_name = self.mod_name(&found).map(|(f, i)| (f, Some(i)));
            }
            _ => {
                let (found, rest) = self.scan(&line, &r.mod_name_list, true, 1);
                line = rest;
                mod_name = self.mod_name(&found).map(|(f, i)| (f, Some(i)));
            }
        }
        if order == 1 && skill_tag.is_none() {
            let (found, rest) = self.scan(&line, &r.skill_name_list, false, 8);
            skill_tag = Self::value_of(&r.skill_name_list, &found).cloned();
            line = rest;
        }

        // Flags
        let (found, rest) = self.scan(&line, &r.mod_flag_list, true, 2);
        let mut mod_flag = Self::value_of(&r.mod_flag_list, &found).cloned();
        line = rest;

        // Value and type according to form
        let cap0 = val::nth(&form_cap, 0);
        let mut mod_value = match val::tonumber(&cap0) {
            V::Nil => cap0.clone(),
            x => x,
        };
        let mut mod_type = val::s("BASE");
        let mut mod_suffix: Option<Rc<str>> = None;
        let mut extra_tags = ExtraTags::None;
        let scan_suffix = |line: &[u8]| -> (Option<Rc<str>>, Vec<u8>) {
            let (found, rest) = self.scan(line, &r.suffix_types, true, 6);
            (Self::value_of(&r.suffix_types, &found).cloned(), rest)
        };
        let cap_text = |i: usize| match val::nth(&form_cap, i) {
            V::Str(s) => Some(text(&s)),
            _ => None,
        };
        let damage = |i: usize| cap_text(i).and_then(|c| r.dmg_types.get(&c).cloned());
        let keyword = |name: &str| self.ctx.field("KeywordFlag", name);
        let kw_frag = |v: V| Frag {
            table: true,
            keyword_flags: (val::to_number(&v)
                .expect("keyword flags")
                .rem_euclid(4294967296.0) as u64 as u32),
            ..Frag::default()
        };
        match form {
            Form::Inc => mod_type = val::s("INC"),
            Form::Red => {
                mod_value = val::neg(&mod_value);
                mod_type = val::s("INC");
            }
            Form::More => mod_type = val::s("MORE"),
            Form::Less => {
                mod_value = val::neg(&mod_value);
                mod_type = val::s("MORE");
            }
            Form::Base | Form::Gain | Form::GrantsGlobal => (mod_suffix, line) = scan_suffix(&line),
            Form::Lose => {
                mod_value = val::neg(&mod_value);
                (mod_suffix, line) = scan_suffix(&line);
            }
            Form::Grants => {
                extra_tags = ExtraTags::Frag(Box::new(hand_attack_tag()));
                (mod_suffix, line) = scan_suffix(&line);
            }
            Form::Removes => {
                mod_value = val::neg(&mod_value);
                extra_tags = ExtraTags::Frag(Box::new(hand_attack_tag()));
                (mod_suffix, line) = scan_suffix(&line);
            }
            Form::Chance | Form::Override | Form::Pen | Form::BaseCost | Form::TotalCost => {
                if form == Form::Override {
                    mod_type = val::s("OVERRIDE");
                }
            }
            Form::RegenPercent | Form::RegenFlat | Form::DegenPercent | Form::DegenFlat => {
                let types = match form {
                    Form::RegenPercent | Form::RegenFlat => &r.regen_types,
                    _ => &r.degen_types,
                };
                mod_name = cap_text(1)
                    .and_then(|c| types.get(&c).cloned())
                    .map(|f| (f, None));
                if matches!(form, Form::RegenPercent | Form::DegenPercent) {
                    mod_suffix = Some("Percent".into());
                }
            }
            Form::Degen => {
                let Some(dt) = damage(1) else {
                    return (Some(Ok(vec![])), Some(text(&line)));
                };
                mod_name = Some((Frag::name(&format!("{dt}Degen")), None));
                mod_suffix = Some("".into());
            }
            Form::Dmg | Form::DmgAttacks | Form::DmgSpells | Form::DmgBoth => {
                let Some(dt) = damage(2) else {
                    return (Some(Ok(vec![])), Some(text(&line)));
                };
                mod_value = val::tbl(
                    vec![],
                    vec![
                        val::tonumber(&val::nth(&form_cap, 0)),
                        val::tonumber(&val::nth(&form_cap, 1)),
                    ],
                );
                mod_name = Some((
                    Frag::names(vec![format!("{dt}Min").into(), format!("{dt}Max").into()]),
                    None,
                ));
                if mod_flag.is_none() {
                    mod_flag = match form {
                        Form::DmgAttacks => Some(kw_frag(keyword("Attack"))),
                        Form::DmgSpells => Some(kw_frag(keyword("Spell"))),
                        Form::DmgBoth => {
                            Some(kw_frag(val::bor(&[keyword("Attack"), keyword("Spell")])))
                        }
                        _ => None,
                    };
                }
            }
            Form::Flag => {
                let is_table = matches!(mod_value, V::Tab(_));
                let name = if is_table {
                    val::field(&mod_value, "name")
                } else {
                    mod_value.clone()
                };
                mod_name = Some((Frag::name(&text(&val::to_bytes(&name))), None));
                mod_type = if is_table {
                    val::field(&mod_value, "type")
                } else {
                    val::s("FLAG")
                };
                mod_value = if is_table {
                    val::field(&mod_value, "value")
                } else {
                    V::Bool(true)
                };
            }
            Form::Doubled => {
                if let Some((name, index)) = &mut mod_name {
                    let first_name = name
                        .names
                        .first()
                        .cloned()
                        .expect("attempt to concatenate a nil value");
                    let second: Rc<str> = format!("Multiplier:{first_name}Doubled").into();
                    if name.table {
                        match name.names.len() {
                            0 | 1 => name.names.push(second.clone()),
                            _ => name.names[1] = second.clone(),
                        }
                        if let Some(i) = index {
                            self.doubled.write().expect("doubled").insert(*i, second);
                        }
                    } else {
                        *name = Frag::names(vec![first_name.clone(), second]);
                    }
                    mod_type = val::tbl(vec![], vec![val::s("MORE"), val::s("OVERRIDE")]);
                    mod_value = val::tbl(vec![], vec![val::n(100.0), val::n(1.0)]);
                    let tag = val::tbl(
                        vec![
                            (val::s("type"), val::s("Multiplier")),
                            (val::s("var"), val::s(&format!("{first_name}Doubled"))),
                            (val::s("globalLimit"), val::n(100.0)),
                            (
                                val::s("globalLimitKey"),
                                val::s(&format!("{first_name}DoubledLimit")),
                            ),
                        ],
                        vec![],
                    );
                    let tag = Converter::default()
                        .tag(&tag)
                        .unwrap_or_else(|e| panic!("ModParser: {e}"));
                    extra_tags = ExtraTags::PerMod(vec![vec![tag]]);
                }
            }
        }

        let Some((mod_name, _)) = mod_name else {
            return (Some(Ok(vec![])), Some(text(&line)));
        };

        // Combine flags and tags
        let mut flags = 0u32;
        let mut keyword_flags = 0u32;
        let mut tag_list: Vec<Tag> = Vec::new();
        let mut mod_tag_list: Option<&Vec<Vec<Tag>>> = None;
        let mut misc = Frag::default();
        let extra_frag = match &extra_tags {
            ExtraTags::Frag(f) => Some(&**f),
            _ => None,
        };
        for d in [
            Some(&mod_name),
            pre_flag.as_ref(),
            mod_flag.as_ref(),
            mod_tag.as_ref(),
            mod_tag2.as_ref(),
            skill_tag.as_ref(),
            extra_frag,
        ]
        .into_iter()
        .flatten()
        {
            if !d.table {
                continue;
            }
            flags |= d.flags;
            keyword_flags |= d.keyword_flags;
            if let Some(t) = &d.tag {
                tag_list.push(t.clone());
            } else if let Some(l) = &d.tag_list {
                tag_list.extend(l.iter().cloned());
            }
            d.merge_into(&mut misc);
        }
        if let ExtraTags::PerMod(l) = &extra_tags {
            mod_tag_list = Some(l);
        }

        // Generate the mod list
        let suffix = mod_suffix
            .or_else(|| misc.mod_suffix.clone())
            .unwrap_or_else(|| "".into());
        let pick = |v: &V, i: usize| match v {
            V::Tab(t) => t.get_int(i as i64 + 1).clone(),
            v => v.clone(),
        };
        let build = || -> Mods {
            let conv = Converter::default();
            let mut list = Vec::new();
            for (i, name) in mod_name.names.iter().enumerate() {
                let typ = match pick(&mod_type, i) {
                    V::Str(s) => ModType::new(&text(&s)),
                    v => return Err(format!("mod type is a {} value", v.type_name())),
                };
                let value = conv.value(&pick(&mod_value, i))?;
                let mut tags = tag_list.clone();
                if let Some(l) = mod_tag_list
                    && let Some(extra) = l.get(i)
                {
                    match extra.len() {
                        1 => tags.push(extra[0].clone()),
                        0 => panic!("wrong number of arguments to 'insert'"),
                        _ => panic!("table.insert with a position in parseMod"),
                    }
                }
                list.push(Mod {
                    name: Sym::new(&format!("{name}{suffix}")),
                    typ,
                    value,
                    flags,
                    keyword_flags,
                    source: None,
                    source_slot: None,
                    tags: tags.into(),
                    unreached_tags: vec![],
                });
            }
            Ok(list)
        };
        let mods = build().map(|list| {
            let truthy = |b: Option<bool>| b == Some(true);
            let wrap_all =
                |list: Vec<Mod>, f: &dyn Fn(Mod) -> Mod| list.into_iter().map(f).collect();
            if list.is_empty() {
                list
            } else if truthy(misc.add_to_aura) {
                let banner: Vec<Tag> = truthy(misc.only_add_to_banners)
                    .then(|| {
                        let t = val::tbl(
                            vec![
                                (val::s("type"), val::s("SkillType")),
                                (val::s("skillType"), self.ctx.field("SkillType", "Banner")),
                            ],
                            vec![],
                        );
                        Converter::default()
                            .tag(&t)
                            .unwrap_or_else(|e| panic!("ModParser: {e}"))
                    })
                    .into_iter()
                    .collect();
                wrap_all(list, &|m| {
                    wrap("ExtraAuraEffect", m, vec![], banner.clone())
                })
            } else if truthy(misc.new_aura) {
                wrap_all(list, &|mut m| {
                    let tags: Vec<Tag> = std::mem::take(&mut *m.tags);
                    let extra = misc
                        .new_aura_only_allies
                        .map(|b| ("onlyAllies", ModValue::Bool(b)))
                        .into_iter()
                        .collect();
                    wrap("ExtraAura", m, extra, tags)
                })
            } else if truthy(misc.add_to_minion) {
                let mut tags = vec![];
                tags.extend(misc.player_tag.iter().cloned());
                tags.extend(misc.add_to_minion_tag.iter().cloned());
                tags.extend(misc.player_tag_list.iter().flatten().cloned());
                wrap_all(list, &|m| wrap("MinionModifier", m, vec![], tags.clone()))
            } else if let Some(skill) = &misc.add_to_skill {
                wrap_all(list, &|m| {
                    wrap("ExtraSkillMod", m, vec![], vec![skill.clone()])
                })
            } else if truthy(misc.apply_to_enemy) {
                let mut tags = vec![];
                tags.extend(misc.player_tag.iter().cloned());
                tags.extend(misc.player_tag_list.iter().flatten().cloned());
                wrap_all(list, &|mut m| {
                    if truthy(misc.actor_enemy)
                        && let Some(t) = m.tags.first_mut()
                    {
                        set_actor(t, "enemy");
                    }
                    wrap("EnemyModifier", m, vec![], tags.clone())
                })
            } else {
                list
            }
        });
        let rest = line
            .iter()
            .any(|c| !matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
            .then(|| text(&line));
        (Some(mods), rest)
    }
}
