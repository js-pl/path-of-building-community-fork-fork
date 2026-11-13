//! The Party tab (`Classes/PartyTab.lua`): the buffs of other party members
//! a build is calculated with, read from the build's `<Party>` section
//! (`PartyTab:Load`, `ParseBuffs`), and the buffs a support build exports
//! for its party (`setBuffExports`, `exportBuffs`) when the Import tab's
//! "Export Support" is on. Mods travel as text, `modLib.formatSourceMod`
//! and `modLib.parseFormattedSourceMod` (`Modules/ModTools.lua`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::OutVal;
use crate::mods::convert::{Converter, DKey, Dyn, Kind};
use crate::mods::tag::{Names, Sockets, Tag, TagKind};
use crate::mods::{Mod, ModType, ModValue, Record, Sym};
use crate::pat;
use crate::xml::{Element, Node};
use std::borrow::Cow;
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, String>;

/// A party member's buff (`{ modList, effectMult, isMark }`)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyBuff {
    pub mods: Vec<Mod>,
    /// `effectMult`: a number, or nil when the text's was none
    pub effect_mult: Option<f64>,
    /// The `effectMult` of `otherEffects`, which stays text in upstream
    pub effect_text: Option<String>,
    pub is_mark: bool,
}

impl PartyBuff {
    /// `effectMult` in arithmetic
    pub fn effect(&self) -> Result<f64> {
        match &self.effect_text {
            Some(t) => pat::to_number(t),
            None => self.effect_mult,
        }
        .ok_or_else(|| "attempt to perform arithmetic on a nil value (effectMult)".into())
    }
}

/// The buffs of one kind (`list[modType]`): by name, in `pairs` order, and
/// the Vaal skills' under `Vaal`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuffGroup {
    pub buffs: Vec<(String, PartyBuff)>,
    pub vaal: Option<Vec<(String, PartyBuff)>>,
}

impl BuffGroup {
    pub fn get(&self, name: &str) -> Option<&PartyBuff> {
        self.buffs.iter().find(|(n, _)| n == name).map(|(_, b)| b)
    }
}

/// One of the tab's buff lists (`actor.Aura`, `Curse`, `Warcry`, `Link`):
/// its groups by mod type (`Aura`, `AuraDebuff`, `extraAura`, ...)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuffList {
    pub groups: Vec<(String, BuffGroup)>,
    /// `list.limit`, the curse limit
    pub limit: Option<f64>,
}

impl BuffList {
    pub fn group(&self, typ: &str) -> Option<&BuffGroup> {
        self.groups.iter().find(|(t, _)| t == typ).map(|(_, g)| g)
    }

    fn group_mut(&mut self, typ: &str) -> &mut BuffGroup {
        let i = match self.groups.iter().position(|(t, _)| t == typ) {
            Some(i) => i,
            None => {
                self.groups.push((typ.to_string(), BuffGroup::default()));
                self.groups.len() - 1
            }
        };
        &mut self.groups[i].1
    }
}

/// The Party tab of a build
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Party {
    /// `actor.modDB`: the party members' stats as flags
    pub member_mods: Vec<Mod>,
    /// `actor.output`: the party members' stats as numbers
    pub member_output: BTreeMap<String, OutVal>,
    pub aura: BuffList,
    pub curse: BuffList,
    pub warcry: BuffList,
    pub link: BuffList,
    /// `enemyModList`
    pub enemy_mods: Vec<Mod>,
    /// `enableExportBuffs`, the Import tab's `exportParty`
    pub export_buffs: bool,
    /// The texts the tab was loaded with (`ImportedBuffs`), by name
    pub texts: Vec<(String, String)>,
}

/// What the text of an element is (`node[1]`)
fn first_text(e: &Element) -> &str {
    match e.children.first() {
        Some(Node::Text(t)) => t,
        _ => "",
    }
}

impl Party {
    /// `PartyTab:Load` with the Import tab's `exportParty`. `skill`
    /// tells whether `data.skills` has a skill id.
    pub fn load(
        party: Option<&Element>,
        export: bool,
        mf: &dyn Fn(&str) -> Option<u32>,
        kf: &dyn Fn(&str) -> Option<u32>,
        skill: &dyn Fn(&str) -> bool,
    ) -> Result<Party> {
        let mut p = Party {
            export_buffs: export,
            ..Party::default()
        };
        let ctx = Ctx { mf, kf, skill };
        for e in party.iter().flat_map(|x| x.elements()) {
            if e.elem != "ImportedBuffs" {
                continue;
            }
            let Some(name) = e.attr("name") else { continue };
            let text = first_text(e);
            match name {
                "PartyMemberStats" => p.parse_member_stats(text)?,
                "Aura" => parse_list(&mut p.aura, text, "Aura", &ctx)?,
                "Curse" => parse_list(&mut p.curse, text, "Curse", &ctx)?,
                "Warcry Skills" => parse_list(&mut p.warcry, text, "Warcry", &ctx)?,
                "Link Skills" => parse_list(&mut p.link, text, "Link", &ctx)?,
                "EnemyConditions" => {
                    for line in lines(text) {
                        if !line.is_empty() {
                            let name = pat::gsub(&line, "Condition:", "Condition:Party:");
                            p.enemy_mods
                                .push(Mod::new(&name, ModType::Flag, true, Some("Party")));
                        }
                    }
                }
                "EnemyMods" => {
                    for line in lines(text) {
                        if line.contains('|')
                            && let Some(mut m) = ctx.parse_mod(&line)?
                        {
                            let s = format!("Party{}", m.source.as_deref().unwrap_or(""));
                            m.source = Some(s.into());
                            p.enemy_mods.push(m);
                        }
                    }
                }
                _ => continue,
            }
            p.texts.push((name.to_string(), text.to_string()));
        }
        Ok(p)
    }

    /// Whether the tab adds anything to a calculation
    pub fn is_empty(&self) -> bool {
        self.member_mods.is_empty()
            && self.member_output.is_empty()
            && self.aura.groups.is_empty()
            && self.curse.groups.is_empty()
            && self.warcry.groups.is_empty()
            && self.link.groups.is_empty()
            && self.enemy_mods.is_empty()
            && !self.export_buffs
    }

    fn parse_member_stats(&mut self, text: &str) -> Result<()> {
        let out = &mut self.member_output;
        for line in lines(text) {
            if line.contains('=') {
                if let Some(c) = pat::captures(&line, "^([%w ]-%w+)%.([%w ]-%w+)=(.+)$") {
                    let table = match out.get(&c[0]) {
                        Some(OutVal::Table(t)) => (**t).clone(),
                        _ => BTreeMap::new(),
                    };
                    let mut table = table;
                    match pat::to_number(&c[2]) {
                        Some(v) => {
                            table.insert(c[1].clone(), OutVal::Num(v));
                        }
                        None => {
                            table.remove(&c[1]);
                        }
                    }
                    out.insert(c[0].clone(), OutVal::Table(table.into()));
                } else if line.contains('|') {
                    let c = pat::captures(&line, "([%w ]-%w+)|(.+)=(.+)")
                        .ok_or("attempt to concatenate a nil value (a party member stat)")?;
                    let mut v = pat::to_number(&c[2]);
                    for tag in pat::gmatch1(&c[1], "([^|]*)|?") {
                        if tag == "percent" {
                            v = Some(
                                v.ok_or("attempt to perform arithmetic on a nil value")? / 100.0,
                            );
                        } else if tag == "max" {
                            let old = out.get(&c[0]).and_then(OutVal::num).unwrap_or(1.0);
                            let n =
                                v.ok_or("bad argument #1 to 'max' (number expected, got nil)")?;
                            v = Some(crate::calc::m_max(n, old));
                        }
                    }
                    set(out, &c[0], v);
                } else {
                    let c = pat::captures(&line, "([%w ]-%w+)=(.+)")
                        .ok_or("table index is nil (a party member stat)")?;
                    set(out, &c[0], pat::to_number(&c[1]));
                }
            } else if !line.is_empty() {
                self.member_mods
                    .push(Mod::new(&line, ModType::Flag, true, Some("Party")));
            }
        }
        Ok(())
    }
}

fn set(out: &mut BTreeMap<String, OutVal>, k: &str, v: Option<f64>) {
    match v {
        Some(v) => {
            out.insert(k.to_string(), OutVal::Num(v));
        }
        None => {
            out.remove(k);
        }
    }
}

/// `buf:gmatch("([^\n]*)\n?")`
fn lines(text: &str) -> Vec<String> {
    pat::gmatch1(text, "([^\n]*)\n?")
}

struct Ctx<'a> {
    mf: &'a dyn Fn(&str) -> Option<u32>,
    kf: &'a dyn Fn(&str) -> Option<u32>,
    skill: &'a dyn Fn(&str) -> bool,
}

/// A value of a mod read from text, as conversion reads Lua values
#[derive(Clone)]
enum P {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Table(Vec<(String, P)>, Vec<P>),
}

impl Dyn for P {
    fn kind(&self) -> Kind<'_, P> {
        match self {
            P::Nil => Kind::Nil,
            P::Bool(b) => Kind::Bool(*b),
            P::Num(n) => Kind::Num(*n),
            P::Str(s) => Kind::Str(Cow::Borrowed(s)),
            P::Table(..) => Kind::Table,
        }
    }

    fn get(&self, k: &str) -> P {
        match self {
            P::Table(h, _) => h
                .iter()
                .find(|(n, _)| n == k)
                .map_or(P::Nil, |(_, v)| v.clone()),
            _ => P::Nil,
        }
    }

    fn get_i(&self, i: i64) -> P {
        match self {
            P::Table(_, a) if i >= 1 => a.get(i as usize - 1).cloned().unwrap_or(P::Nil),
            _ => P::Nil,
        }
    }

    fn border(&self) -> usize {
        match self {
            P::Table(_, a) => a.len(),
            _ => 0,
        }
    }

    fn len_pairs(&self) -> usize {
        match self {
            P::Table(h, a) => h.len() + a.len(),
            _ => 0,
        }
    }

    fn entries(&self) -> Vec<(DKey, P)> {
        match self {
            P::Table(h, a) => a
                .iter()
                .enumerate()
                .map(|(i, v)| (DKey::Int(i as i64 + 1), v.clone()))
                .chain(h.iter().map(|(k, v)| (DKey::Str(k.clone()), v.clone())))
                .collect(),
            _ => vec![],
        }
    }
}

impl Ctx<'_> {
    /// `modLib.parseFormattedSourceMod(line)`
    fn parse_mod(&self, line: &str) -> Result<Option<Mod>> {
        let parts = pat::gmatch1(line, "([^|]*)|?");
        if parts.len() < 4 {
            return Ok(None);
        }
        let value = if parts[0] == "true" {
            P::Bool(true)
        } else {
            P::Num(pat::to_number(&parts[0]).unwrap_or(0.0))
        };
        let flag = |i: usize, f: &dyn Fn(&str) -> Option<u32>| {
            parts.get(i).and_then(|p| f(p)).unwrap_or(0) as f64
        };
        let fields = vec![
            ("value".to_string(), value),
            ("source".to_string(), P::Str(parts[1].clone())),
            ("name".to_string(), P::Str(parts[2].clone())),
            ("type".to_string(), P::Str(parts[3].clone())),
            ("flags".to_string(), P::Num(flag(4, self.mf))),
            ("keywordFlags".to_string(), P::Num(flag(5, self.kf))),
        ];
        let mut tags = parse_tags(parts.get(6).map(String::as_str));
        // Lists come back as text, which upstream fails on when it
        // evaluates the tag: such tags are kept as tags of no kind the
        // typed queries know, which fail likewise
        let mut broken = Vec::new();
        for (i, t) in tags.iter_mut().enumerate() {
            let P::Table(f, _) = t else { continue };
            let text_list = f
                .iter()
                .any(|(k, v)| (k.ends_with("List") || k == "ramp") && matches!(v, P::Str(_)));
            if let Some((_, P::Str(kind))) = f.iter().find(|(k, _)| k == "type")
                && text_list
                && crate::mods::store::EVALUATED_TAGS.contains(&kind.as_str())
            {
                broken.push((i, kind.clone()));
                for e in f.iter_mut().filter(|(k, _)| k == "type") {
                    e.1 = P::Str("PartyTextList".into());
                }
            }
        }
        let m = P::Table(fields, tags);
        let mut m = Converter::default().mod_(&m)?;
        for (i, kind) in broken {
            if let TagKind::Other { kind: k, .. } = &mut m.tags[i].kind {
                *k = Sym::new(&kind);
            }
        }
        Ok(Some(m))
    }
}

/// `modLib.parseTags(line)`
fn parse_tags(line: Option<&str>) -> Vec<P> {
    let Some(line) = line.filter(|l| *l != "-") else {
        return vec![];
    };
    let mut tags = Vec::new();
    for group in pat::gmatch1(line, "([^,]*),?") {
        if group.is_empty() {
            continue;
        }
        let mut set: Vec<(String, P)> = Vec::new();
        for tag in pat::gmatch1(&group, "([^/]*)/?") {
            if tag.is_empty() {
                continue;
            }
            if let Some(c) = pat::captures(&tag, "^(%a+)=(.+)") {
                let v = if c[0] == "threshold" {
                    pat::to_number(&c[1]).map_or(P::Nil, P::Num)
                } else if c[1] == "true" {
                    P::Bool(true)
                } else {
                    P::Str(c[1].clone())
                };
                match set.iter_mut().find(|(k, _)| *k == c[0]) {
                    Some(e) => e.1 = v,
                    None => set.push((c[0].clone(), v)),
                }
            }
        }
        set.retain(|(_, v)| !matches!(v, P::Nil));
        // A tag without a type, which no query looks at
        if !set.iter().any(|(k, _)| k == "type") {
            set.push(("type".into(), P::Str(String::new())));
        }
        tags.push(P::Table(set, vec![]));
    }
    tags
}

/// `ParseBuffs` for the Aura, Curse, Warcry and Link lists
fn parse_list(list: &mut BuffList, text: &str, buff_type: &str, ctx: &Ctx) -> Result<()> {
    #[derive(PartialEq)]
    enum Mode {
        Name,
        CurseLimit,
        Effect,
        IsMark,
        Stats,
    }
    let mut mode = if buff_type == "Curse" {
        Mode::CurseLimit
    } else {
        Mode::Name
    };
    let mut name: Option<String> = None;
    // A number, or the text of an `otherEffects` line
    let mut effect: (Option<f64>, Option<String>) = (None, None);
    let mut is_mark = false;
    let mut mod_type = match buff_type {
        "Link" => "Link",
        "Warcry" => "Warcry",
        _ => "Unknown",
    }
    .to_string();
    for line in lines(text) {
        if line != "---" && line.contains("---") {
            continue;
        }
        if mode == Mode::CurseLimit && !line.is_empty() {
            list.limit = pat::to_number(&line);
            mode = Mode::Name;
        } else if mode == Mode::Name && !line.is_empty() {
            name = Some(line.replace("_Debuff", ""));
            effect = (Some(0.0), None);
            if line == "extraAura" || line == "otherEffects" {
                mod_type = line.clone();
                mode = Mode::Stats;
            } else {
                mode = Mode::Effect;
                mod_type = "Unknown".into();
            }
        } else if mode == Mode::Effect {
            effect = (pat::to_number(&line), None);
            mode = if buff_type == "Curse" {
                Mode::IsMark
            } else {
                Mode::Stats
            };
        } else if mode == Mode::IsMark {
            is_mark = line == "true";
            mode = Mode::Stats;
        } else if line == "---" {
            mode = Mode::Name;
        } else if line.contains('|')
            && name.as_deref() != Some("SKIP")
            && !line.contains("MinionModifier|LIST")
        {
            let mut line = line;
            if mod_type == "otherEffects" {
                let c = pat::captures(&line, "([%w ']-%w+)|(%w+)|(.+)")
                    .ok_or("bad argument #1 to 'gmatch' (string expected, got nil)")?;
                name = Some(c[0].clone());
                effect = (None, Some(c[1].clone()));
                line = c[2].clone();
            }
            let Some(mut m) = ctx.parse_mod(&line)? else {
                continue;
            };
            for t in m.tags.iter() {
                if let TagKind::Other { kind, fields } = &t.kind
                    && kind.as_str() == "GlobalEffect"
                    && !["Link", "Warcry", "otherEffects"].contains(&mod_type.as_str())
                {
                    mod_type = match fields.get("effectType") {
                        Some(ModValue::Str(s)) => s.to_string(),
                        Some(ModValue::Bool(true)) => "true".into(),
                        Some(ModValue::Number(n)) => pat::num(*n),
                        _ => return Err("not ported: a party buff without its effect type".into()),
                    };
                }
            }
            let cur = name
                .clone()
                .ok_or("attempt to index a nil value (a party buff's name)")?;
            let group = list.group_mut(&mod_type);
            let elements = if cur.starts_with("Vaal") {
                group.vaal.get_or_insert_with(Vec::new)
            } else {
                &mut group.buffs
            };
            let fresh = || PartyBuff {
                mods: vec![],
                effect_mult: effect.0,
                effect_text: effect.1.clone(),
                is_mark: false,
            };
            let mut skip = false;
            match elements.iter().position(|(n, _)| *n == cur) {
                None => {
                    let mut b = fresh();
                    b.is_mark = is_mark;
                    elements.push((cur.clone(), b));
                }
                Some(i) => {
                    let old = &elements[i].1;
                    let differs = match (&old.effect_text, &effect.1) {
                        (Some(a), Some(b)) => a != b,
                        (None, None) => old.effect_mult != effect.0,
                        _ => true,
                    };
                    if differs {
                        let less = match (&old.effect_text, &effect.1) {
                            (Some(a), Some(b)) => a.as_bytes() < b.as_bytes(),
                            (None, None) => match (old.effect_mult, effect.0) {
                                (Some(a), Some(b)) => a < b,
                                _ => {
                                    return Err(
                                        "attempt to compare nil with number (effectMult)".into()
                                    );
                                }
                            },
                            _ => return Err("attempt to compare number with string".into()),
                        };
                        if less {
                            elements[i].1 = fresh();
                        } else {
                            skip = true;
                        }
                    }
                }
            }
            if skip {
                name = Some("SKIP".into());
                continue;
            }
            if m.source.as_deref().is_some_and(|s| s.contains("Item")) {
                let s = m.source.as_deref().unwrap_or("");
                let c = pat::captures(s, "Item:(%d+):(.+)")
                    .ok_or("attempt to concatenate a nil value (a party item source)")?;
                m.source = Some(format!("Party - {}", c[1]).into());
            }
            if m.source.as_deref().is_some_and(|s| s.contains("Skill")) {
                let s = m.source.as_deref().unwrap_or("").to_string();
                let id = pat::capture(&s, "Skill:(.+)");
                if !id.as_deref().is_some_and(|i| (ctx.skill)(i)) {
                    let minimised =
                        pat::gsub_fn(&cur, " %l", |c| Some(c[0].to_uppercase())).replace(' ', "");
                    m.source = if (ctx.skill)(&minimised) {
                        Some(format!("Skill:{minimised}").into())
                    } else {
                        id.map(Into::into)
                    };
                }
            }
            if buff_type == "Link" {
                m.name = Sym::new(&m.name.as_str().replace("Parent", "PartyMember"));
                let tags: Vec<Tag> = m
                    .tags
                    .iter()
                    .cloned()
                    .map(|mut t| {
                        set_parent_actor(&mut t);
                        t
                    })
                    .collect();
                m.tags = tags.into();
            }
            let i = elements.iter().position(|(n, _)| *n == cur).unwrap();
            elements[i].1.mods.push(m);
        }
    }
    Ok(())
}

/// `if modTag.actor == "parent" then modTag.actor = "partyMembers"`
fn set_parent_actor(t: &mut Tag) {
    let parent = Sym::new("parent");
    let party = Sym::new("partyMembers");
    let fix = |a: &mut Option<Sym>| {
        if *a == Some(parent) {
            *a = Some(party);
        }
    };
    match &mut t.kind {
        TagKind::Multiplier(m) => fix(&mut m.actor),
        TagKind::MultiplierThreshold(m) => fix(&mut m.actor),
        TagKind::PerStat(m) => fix(&mut m.actor),
        TagKind::PercentStat(m) => fix(&mut m.actor),
        TagKind::Condition { actor, .. } => fix(actor),
        TagKind::ActorCondition { actor, .. } => fix(actor),
        TagKind::Other { fields, .. } => {
            if let Some(ModValue::Str(s)) = fields.get_mut(Sym::new("actor"))
                && &**s == "parent"
            {
                *s = "partyMembers".into();
            }
        }
        _ => {}
    }
}

// Formatting mods as text (`modLib.formatSourceMod`)

/// A tag's fields as upstream's table has them, and its type
fn tag_fields(t: &Tag) -> (String, Vec<(String, ModValue)>) {
    let mut m: Vec<(String, ModValue)> = Vec::new();
    let mut put = |k: &str, v: ModValue| m.push((k.to_string(), v));
    fn names_v(n: &Names) -> (bool, ModValue) {
        match n {
            Names::One(s) => (true, ModValue::Str(s.as_str().into())),
            Names::List(l) => (
                false,
                ModValue::List(l.iter().map(|s| ModValue::Str(s.as_str().into())).collect()),
            ),
        }
    }
    macro_rules! names {
        ($one:expr, $list:expr, $n:expr) => {
            if let Some(n) = $n {
                let (one, v) = names_v(n);
                put(if one { $one } else { $list }, v);
            }
        };
    }
    macro_rules! num {
        ($k:expr, $v:expr) => {
            if let Some(v) = $v {
                put($k, ModValue::Number(v));
            }
        };
    }
    macro_rules! sym {
        ($k:expr, $v:expr) => {
            if let Some(v) = $v {
                put($k, ModValue::Str(v.as_str().into()));
            }
        };
    }
    macro_rules! text {
        ($k:expr, $v:expr) => {
            if let Some(v) = $v {
                put($k, ModValue::Str(v.clone()));
            }
        };
    }
    macro_rules! yes {
        ($k:expr, $v:expr) => {
            if $v {
                put($k, ModValue::Bool(true));
            }
        };
    }
    let nums = |v: &[f64]| ModValue::List(v.iter().map(|n| ModValue::Number(*n)).collect());
    let kind = match &t.kind {
        TagKind::Multiplier(x) => {
            names!("var", "varList", &x.vars);
            sym!("actor", x.actor);
            sym!("limitActor", x.limit_actor);
            num!("div", x.div);
            sym!("divVar", x.div_var);
            yes!("noFloor", x.no_floor);
            num!("limit", x.limit);
            sym!("limitVar", x.limit_var);
            sym!("limitStat", x.limit_stat);
            yes!("limitTotal", x.limit_total);
            yes!("limitNegTotal", x.limit_neg_total);
            yes!("invert", x.invert);
            num!("base", x.base);
            "Multiplier".to_string()
        }
        TagKind::MultiplierThreshold(x) => {
            names!("var", "varList", &x.vars);
            sym!("actor", x.actor);
            sym!("thresholdActor", x.threshold_actor);
            num!("threshold", x.threshold);
            sym!("thresholdVar", x.threshold_var);
            yes!("upper", x.upper);
            yes!("equals", x.equals);
            "MultiplierThreshold".into()
        }
        TagKind::PerStat(x) => {
            names!("stat", "statList", &x.stats);
            sym!("actor", x.actor);
            num!("div", x.div);
            sym!("divVar", x.div_var);
            num!("limit", x.limit);
            sym!("limitVar", x.limit_var);
            yes!("limitTotal", x.limit_total);
            num!("base", x.base);
            "PerStat".into()
        }
        TagKind::PercentStat(x) => {
            names!("stat", "statList", &x.stats);
            sym!("actor", x.actor);
            num!("percent", x.percent);
            sym!("percentVar", x.percent_var);
            yes!("floor", x.floor);
            num!("limit", x.limit);
            sym!("limitVar", x.limit_var);
            yes!("limitTotal", x.limit_total);
            num!("base", x.base);
            "PercentStat".into()
        }
        TagKind::StatThreshold(x) => {
            names!("stat", "statList", &x.stats);
            num!("threshold", x.threshold);
            sym!("thresholdStat", x.threshold_stat);
            num!("thresholdPercent", x.threshold_percent);
            sym!("thresholdPercentVar", x.threshold_percent_var);
            yes!("upper", x.upper);
            "StatThreshold".into()
        }
        TagKind::DistanceRamp(r) => {
            put(
                "ramp",
                ModValue::List(r.iter().map(|(a, b)| nums(&[*a, *b])).collect()),
            );
            "DistanceRamp".into()
        }
        TagKind::MeleeProximity(r) => {
            put("ramp", nums(r));
            "MeleeProximity".into()
        }
        TagKind::Limit { limit, limit_var } => {
            num!("limit", *limit);
            sym!("limitVar", *limit_var);
            "Limit".into()
        }
        TagKind::Condition { vars, neg, actor } => {
            names!("var", "varList", Some(vars));
            yes!("neg", *neg);
            sym!("actor", *actor);
            "Condition".into()
        }
        TagKind::ActorCondition { actor, vars, neg } => {
            sym!("actor", *actor);
            names!("var", "varList", vars);
            yes!("neg", *neg);
            "ActorCondition".into()
        }
        TagKind::ItemCondition(x) => {
            put("itemSlot", ModValue::Str(x.item_slot.clone()));
            yes!("allSlots", x.all_slots);
            yes!("bothSlots", x.both_slots);
            yes!("excludeSelf", x.exclude_self);
            text!("searchCond", &x.search_cond);
            text!("rarityCond", &x.rarity_cond);
            text!("nameCond", &x.name_cond);
            for (k, v) in [
                ("corruptedCond", x.corrupted_cond),
                ("shaperCond", x.shaper_cond),
                ("elderCond", x.elder_cond),
            ] {
                if let Some(v) = v {
                    put(k, ModValue::Bool(v));
                }
            }
            yes!("neg", x.neg);
            "ItemCondition".into()
        }
        TagKind::SocketedIn(x) => {
            sym!("slotName", x.slot_name);
            text!("keyword", &x.keyword);
            text!("socketColor", &x.socket_color);
            match &x.sockets {
                Some(Sockets::All) => put("sockets", ModValue::Str("all".into())),
                Some(Sockets::Numbers(n)) => put("sockets", nums(n)),
                Some(Sockets::Fewer(n)) => put("sockets", ModValue::Number(*n)),
                None => {}
            }
            yes!("neg", x.neg);
            "SocketedIn".into()
        }
        TagKind::SkillName {
            names: n,
            include_transfigured,
            summon_skill,
            neg,
            skill_id,
        } => {
            names!("skillName", "skillNameList", n);
            yes!("includeTransfigured", *include_transfigured);
            yes!("summonSkill", *summon_skill);
            yes!("neg", *neg);
            text!("skillId", skill_id);
            "SkillName".into()
        }
        TagKind::SkillId(id) => {
            text!("skillId", id);
            "SkillId".into()
        }
        TagKind::SkillPart { list, part, neg } => {
            if let Some(l) = list {
                put("skillPartList", nums(l));
            }
            num!("skillPart", *part);
            yes!("neg", *neg);
            "SkillPart".into()
        }
        TagKind::SkillType {
            list,
            skill_type,
            neg,
        } => {
            if let Some(l) = list {
                put("skillTypeList", nums(l));
            }
            num!("skillType", *skill_type);
            yes!("neg", *neg);
            "SkillType".into()
        }
        TagKind::BaseFlag { flag, neg } => {
            put("baseFlag", ModValue::Str(flag.as_str().into()));
            yes!("neg", *neg);
            "BaseFlag".into()
        }
        TagKind::SlotName { names: n, neg } => {
            names!("slotName", "slotNameList", n);
            yes!("neg", *neg);
            "SlotName".into()
        }
        TagKind::ModFlagOr(f) => {
            put("modFlags", ModValue::Number(*f as f64));
            "ModFlagOr".into()
        }
        TagKind::KeywordFlagAnd(f) => {
            put("keywordFlags", ModValue::Number(*f as f64));
            "KeywordFlagAnd".into()
        }
        TagKind::MonsterTag { names: n, neg } => {
            names!("monsterTag", "monsterTagList", n);
            yes!("neg", *neg);
            "MonsterTag".into()
        }
        TagKind::Other { kind, fields } => {
            for (k, v) in &fields.fields {
                put(k.as_str(), v.clone());
            }
            kind.as_str().to_string()
        }
    };
    if let Some((limit, key)) = t.global_limit {
        put("globalLimit", ModValue::Number(limit));
        put("globalLimitKey", ModValue::Str(key.as_str().into()));
    }
    (kind, m)
}

/// Lua's `tostring` of a mod value that is not a table
fn tostring(v: &ModValue) -> Result<String> {
    Ok(match v {
        ModValue::Bool(b) => b.to_string(),
        ModValue::Number(n) => pat::num(*n),
        ModValue::Str(s) => s.to_string(),
        _ => return Err("not ported: formatting a table".into()),
    })
}

/// Sorted parameter names, `type` first (`formatTag`, `formatValue`)
fn sorted(fields: &[(String, ModValue)], have_type: bool) -> Vec<&(String, ModValue)> {
    let mut out: Vec<&(String, ModValue)> = fields.iter().collect();
    out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if have_type {
        out.retain(|f| f.0 != "type");
    }
    out
}

/// `modLib.formatTag` of a table with these fields (and the type, if any)
fn format_fields(kind: Option<&str>, fields: &[(String, ModValue)]) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(k) = kind {
        parts.push(format!("type={k}"));
    }
    for (k, v) in sorted(fields, kind.is_some()) {
        let v = match v {
            ModValue::List(l) if l.is_empty() => "{}".to_string(),
            ModValue::List(l) => match &l[0] {
                ModValue::Record(_) | ModValue::List(_) | ModValue::Mod(_) => {
                    return Err("not ported: formatting a list of tags".into());
                }
                _ => format!(
                    "{{{}}}",
                    l.iter()
                        .map(tostring)
                        .collect::<Result<Vec<_>>>()?
                        .join(",")
                ),
            },
            ModValue::Record(r) => format!("{{{}}}", format_record(r)?),
            v => tostring(v)?,
        };
        parts.push(format!("{k}={v}"));
    }
    Ok(parts.join("/"))
}

fn format_record(r: &Record) -> Result<String> {
    let fields: Vec<(String, ModValue)> = r
        .fields
        .iter()
        .filter(|(k, _)| k.as_str() != "type")
        .map(|(k, v)| (k.as_str().to_string(), v.clone()))
        .collect();
    let kind = r.get("type").map(tostring).transpose()?;
    format_fields(kind.as_deref(), &fields)
}

/// `modLib.formatTag`
pub fn format_tag(t: &Tag) -> Result<String> {
    let (kind, fields) = tag_fields(t);
    format_fields(Some(&kind), &fields)
}

/// `ModFlag` and `KeywordFlag`, by name
pub struct Flags<'a> {
    pub mod_flags: &'a std::collections::HashMap<String, u32>,
    pub keyword_flags: &'a std::collections::HashMap<String, u32>,
}

/// `modLib.formatValue`
fn format_value(v: &ModValue, fl: &Flags) -> Result<String> {
    let entries: Vec<(String, ModValue)> = match v {
        ModValue::Record(r) => r
            .fields
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect(),
        ModValue::List(l) => {
            // Number keys sort as numbers
            let parts: Vec<String> = l
                .iter()
                .enumerate()
                .map(|(i, x)| Ok(format!("{}={}", i + 1, format_value(x, fl)?)))
                .collect::<Result<_>>()?;
            return Ok(format!("{{{}}}", parts.join("/")));
        }
        ModValue::Mod(_) => return Err("not ported: formatting a mod as a value".into()),
        ModValue::Func(_) => return Err("not ported: formatting a function".into()),
        v => return tostring(v),
    };
    let have_type = entries.iter().any(|(k, _)| k == "type");
    let mut parts = Vec::new();
    if let Some((_, t)) = entries.iter().find(|(k, _)| k == "type") {
        parts.push(format!("type={}", format_value(t, fl)?));
    }
    for (k, x) in sorted(&entries, have_type) {
        match (k.as_str(), x) {
            ("mod", ModValue::Mod(m)) => parts.push(format!("mod=[{}]", format_mod(m, fl)?)),
            _ => parts.push(format!("{k}={}", format_value(x, fl)?)),
        }
    }
    Ok(format!("{{{}}}", parts.join("/")))
}

fn format_mod_params(m: &Mod, fl: &Flags) -> Result<String> {
    let tags = if m.tags.is_empty() {
        "-".to_string()
    } else {
        m.tags
            .iter()
            .map(format_tag)
            .collect::<Result<Vec<_>>>()?
            .join(",")
    };
    Ok(format!(
        "{}|{}|{}|{}|{tags}",
        m.name.as_str(),
        m.typ.name(),
        format_flags(m.flags, fl.mod_flags),
        format_flags(m.keyword_flags, fl.keyword_flags)
    ))
}

/// `modLib.formatMod`
fn format_mod(m: &Mod, fl: &Flags) -> Result<String> {
    Ok(format!(
        "{} = {}",
        format_value(&m.value, fl)?,
        format_mod_params(m, fl)?
    ))
}

/// `modLib.formatFlags(flags, table)`: the names whose bits are all set,
/// sorted, or `-`
pub fn format_flags(flags: u32, table: &std::collections::HashMap<String, u32>) -> String {
    let mut names: Vec<&str> = table
        .iter()
        .filter(|(_, v)| flags & **v == **v)
        .map(|(k, _)| k.as_str())
        .collect();
    names.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    if names.is_empty() {
        "-".into()
    } else {
        names.join(",")
    }
}

/// `modLib.formatSourceMod`
pub fn format_source_mod(m: &Mod, fl: &Flags) -> Result<String> {
    Ok(format!(
        "{}|{}|{}",
        format_value(&m.value, fl)?,
        m.source.as_deref().unwrap_or("nil"),
        format_mod_params(m, fl)?
    ))
}

// The buffs a build exports for its party (`buffExports`)

/// An exported buff (`{ effectMult, modList, isMark }`)
#[derive(Clone, Debug, PartialEq)]
pub struct ExportBuff {
    pub effect_mult: f64,
    pub mods: Vec<Mod>,
    pub is_mark: bool,
}

/// An entry of the exported auras: a buff, or the buffs under
/// `otherEffects`
#[derive(Clone, Debug, PartialEq)]
pub enum AuraExport {
    Buff(ExportBuff),
    Other(Vec<(String, ExportBuff)>),
}

/// `buffExports`, what `calcs.perform` exports when the build exports its
/// buffs
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuffExports {
    pub aura: Vec<(String, AuraExport)>,
    pub curse: Vec<(String, ExportBuff)>,
    pub curse_limit: f64,
    pub warcry: Vec<(String, ExportBuff)>,
    pub link: Vec<(String, ExportBuff)>,
    /// By mod name: one mod, or several (`MultiStat`)
    pub enemy_mods: Vec<(String, Vec<Mod>, bool)>,
    pub enemy_conditions: Vec<String>,
    pub player_mods: Vec<String>,
}

impl BuffExports {
    pub(crate) fn set_aura(&mut self, name: String, b: AuraExport) {
        match self.aura.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 = b,
            None => self.aura.push((name, b)),
        }
    }

    pub(crate) fn aura_has(&self, name: &str) -> bool {
        self.aura.iter().any(|(n, _)| n == name)
    }

    pub(crate) fn aura_get(&self, name: &str) -> Option<&AuraExport> {
        self.aura.iter().find(|(n, _)| n == name).map(|(_, b)| b)
    }

    pub(crate) fn remove_aura(&mut self, name: &str) {
        self.aura.retain(|(n, _)| n != name);
    }

    pub(crate) fn set_other_effect(&mut self, name: &str, b: ExportBuff) {
        if !self.aura_has("otherEffects") {
            self.aura
                .push(("otherEffects".into(), AuraExport::Other(Vec::new())));
        }
        if let Some((_, AuraExport::Other(l))) =
            self.aura.iter_mut().find(|(n, _)| n == "otherEffects")
        {
            match l.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 = b,
                None => l.push((name.to_string(), b)),
            }
        }
    }

    /// `PartyTab:exportBuffs(buffType)` for each of the `ExportedBuffs`
    /// a build saves, by name; empty texts are left out
    pub fn texts(&self, fl: &Flags) -> Result<Vec<(&'static str, String)>> {
        let fmt = |m: &Mod| format_source_mod(m, fl);
        fn sorted(l: &[(String, ExportBuff)]) -> Vec<&(String, ExportBuff)> {
            let mut v: Vec<&(String, ExportBuff)> = l.iter().collect();
            v.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            v
        }
        let mut out = Vec::new();
        // PlayerMods and EnemyConditions: the names
        let names = |list: &[String]| {
            let mut v: Vec<&String> = list.iter().collect();
            v.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
        };
        out.push(("PartyMemberStats", names(&self.player_mods)));
        // Aura
        let mut buf = String::new();
        let mut auras: Vec<&(String, AuraExport)> = self.aura.iter().collect();
        auras.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for (name, entry) in auras {
            if let (true, AuraExport::Buff(b)) = (name == "extraAura", entry)
                && b.mods.is_empty()
            {
                continue;
            }
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(name);
            match entry {
                AuraExport::Other(l) if name == "otherEffects" => {
                    for (inner, b) in sorted(l) {
                        for m in &b.mods {
                            buf.push_str(&format!(
                                "\n{inner}|{}|{}",
                                pat::num(b.effect_mult * 100.0),
                                fmt(m)?
                            ));
                        }
                    }
                }
                AuraExport::Other(_) => {
                    return Err("not ported: exported buffs under another name".into());
                }
                AuraExport::Buff(b) => {
                    if name != "extraAura" && name != "otherEffects" {
                        buf.push_str(&format!("\n{}", pat::num(b.effect_mult * 100.0)));
                    }
                    for m in &b.mods {
                        buf.push_str(&format!("\n{}", fmt(m)?));
                    }
                }
            }
            buf.push_str("\n---");
        }
        out.push(("Aura", buf));
        // Curse
        let mut buf = format!("--- Curse Limit ---\n{}", pat::num(self.curse_limit));
        for (name, b) in sorted(&self.curse) {
            buf.push_str(&format!("\n{name}\n{}", pat::num(b.effect_mult * 100.0)));
            buf.push_str(if b.is_mark { "\ntrue" } else { "\nfalse" });
            for m in &b.mods {
                buf.push_str(&format!("\n{}", fmt(m)?));
            }
            buf.push_str("\n---");
        }
        out.push(("Curse", buf));
        for (key, list) in [("Warcry Skills", &self.warcry), ("Link Skills", &self.link)] {
            let mut buf = String::new();
            for (name, b) in sorted(list) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&format!("{name}\n{}", pat::num(b.effect_mult * 100.0)));
                for m in &b.mods {
                    buf.push_str(&format!("\n{}", fmt(m)?));
                }
                buf.push_str("\n---");
            }
            out.push((key, buf));
        }
        out.push(("EnemyConditions", names(&self.enemy_conditions)));
        let mut enemy: Vec<&(String, Vec<Mod>, bool)> = self.enemy_mods.iter().collect();
        enemy.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut buf = String::new();
        for (name, mods, _) in enemy {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(name);
            for m in mods {
                buf.push_str(&format!("\n{}", fmt(m)?));
            }
        }
        out.push(("EnemyMods", buf));
        out.retain(|(_, t)| !t.is_empty());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> std::collections::HashMap<String, u32> {
        [("Attack", 1), ("Spell", 2), ("Hit", 4)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn mods_round_trip() {
        let f = flags();
        let mf = |n: &str| f.get(n).copied();
        let ctx = Ctx {
            mf: &mf,
            kf: &mf,
            skill: &|_| false,
        };
        let line = "10|Skill:X|Damage|INC|Attack|-|type=GlobalEffect/effectType=Aura,type=Multiplier/limit=5/var=Foo";
        let m = ctx.parse_mod(line).unwrap().unwrap();
        assert_eq!(m.flags, 1);
        let fl = Flags {
            mod_flags: &f,
            keyword_flags: &f,
        };
        assert_eq!(format_source_mod(&m, &fl).unwrap(), line);
        // Several flags are no single name of `ModFlag`
        let m = ctx
            .parse_mod("1|S|Damage|MORE|Attack,Hit|-|-")
            .unwrap()
            .unwrap();
        assert_eq!(m.flags, 0);
        assert_eq!(format_flags(5, &f), "Attack,Hit");
    }

    #[test]
    fn text_lists_parse() {
        let f = flags();
        let mf = |n: &str| f.get(n).copied();
        let ctx = Ctx {
            mf: &mf,
            kf: &mf,
            skill: &|_| false,
        };
        let line = "-50|Tree:1|LightningResist|BASE|-|-|type=Condition/neg=true/var=A,type=Condition/varList={B,C}";
        let m = ctx.parse_mod(line).unwrap().unwrap();
        assert_eq!(m.tags.len(), 3);
        assert!(
            matches!(&m.tags[1].kind, TagKind::Other { kind, .. } if kind.as_str() == "Condition")
        );
        assert!(matches!(&m.tags[2].kind, TagKind::Other { kind, .. } if kind.as_str().is_empty()));
    }

    #[test]
    fn auras_parse() {
        let f = flags();
        let mf = |n: &str| f.get(n).copied();
        let ctx = Ctx {
            mf: &mf,
            kf: &mf,
            skill: &|_| false,
        };
        let text = "Grace\n150\n1|Skill:Grace|Evasion|BASE|-|-|type=GlobalEffect/effectType=Aura\n---\nVaal Grace\n100\n1|Skill:VaalGrace|Evasion|BASE|-|-|type=GlobalEffect/effectType=Aura\n---\nGrace\n120\n9|Skill:Grace|Evasion|BASE|-|-|type=GlobalEffect/effectType=Aura\n---";
        let mut list = BuffList::default();
        parse_list(&mut list, text, "Aura", &ctx).unwrap();
        let g = list.group("Aura").unwrap();
        assert_eq!(g.buffs.len(), 1);
        assert_eq!(g.buffs[0].1.effect_mult, Some(150.0));
        assert_eq!(g.buffs[0].1.mods.len(), 1);
        assert_eq!(g.vaal.as_ref().unwrap().len(), 1);
    }
}
