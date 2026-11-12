//! The Calcs tab's calculation breakdowns (`Modules/CalcBreakdown.lua` and
//! the `breakdown.*` tables the calculations write), for the player and a
//! minion. An actor has breakdowns only in the sidebar's and the Calcs
//! tab's calculations (`PerformInput::breakdown`, `env.buildBreakdown`);
//! in calculators and Full DPS nothing of this runs.
//!
//! Not ported yet: `breakdown.leech` and the breakdowns of the triggers,
//! mirages and offence.
//!
//! In sync with upstream `16de4b82d57f1c0de6eb40f37143c32d4da36a02`.

use crate::calc::Result;
use crate::mods::{Mod, ModValue};
use std::collections::HashMap;

/// `colorCodes` of `Data/Global.lua` that the breakdowns use
pub mod color {
    pub const NEGATIVE: &str = "^xDD0022";
    pub const POSITIVE: &str = "^x33FF77";
    pub const GEM: &str = "^x1AA29B";

    /// `colorCodes[rarity]`
    pub fn rarity(r: &str) -> Option<&'static str> {
        Some(match r {
            "NORMAL" => "^xC8C8C8",
            "MAGIC" => "^x8888FF",
            "RARE" => "^xFFFF77",
            "UNIQUE" => "^xAF6025",
            "RELIC" => "^x60C060",
            _ => return None,
        })
    }
}

/// An argument of [`lua_format`]
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Num(f64),
    Str(&'a str),
}

impl From<f64> for Arg<'_> {
    fn from(n: f64) -> Self {
        Arg::Num(n)
    }
}

impl<'a> From<&'a str> for Arg<'a> {
    fn from(s: &'a str) -> Self {
        Arg::Str(s)
    }
}

impl<'a> From<&'a String> for Arg<'a> {
    fn from(s: &'a String) -> Self {
        Arg::Str(s)
    }
}

/// `string.format(fmt, ...)` as the Lua runtime formats
#[macro_export]
macro_rules! sf {
    ($f:expr $(, $a:expr)* $(,)?) => {
        $crate::breakdown::lua_format($f, &[$($crate::breakdown::Arg::from($a)),*])
    };
}

fn pad(sign: &str, digits: &str, width: Option<usize>, flags: &str, zero_ok: bool) -> String {
    let len = sign.len() + digits.len();
    match width {
        Some(w) if len < w => {
            let fill = w - len;
            if flags.contains('-') {
                format!("{sign}{digits}{}", " ".repeat(fill))
            } else if flags.contains('0') && zero_ok {
                format!("{sign}{}{digits}", "0".repeat(fill))
            } else {
                format!("{}{sign}{digits}", " ".repeat(fill))
            }
        }
        _ => format!("{sign}{digits}"),
    }
}

/// `string.format(fmt, ...)`: `%d`, `%f`, `%g`, `%e`, `%s` and `%%` with
/// flags, width and precision, as the Lua runtime formats them
pub fn lua_format(fmt: &str, args: &[Arg]) -> String {
    let f = fmt.as_bytes();
    let mut out = String::new();
    let mut args = args.iter();
    let mut i = 0;
    while i < f.len() {
        let start = i;
        while i < f.len() && f[i] != b'%' {
            i += 1;
        }
        out.push_str(&fmt[start..i]);
        if i >= f.len() {
            break;
        }
        i += 1;
        if f.get(i) == Some(&b'%') {
            out.push('%');
            i += 1;
            continue;
        }
        let fl = i;
        while i < f.len() && b"-+ #0".contains(&f[i]) {
            i += 1;
        }
        let flags = &fmt[fl..i];
        let w = i;
        while i < f.len() && f[i].is_ascii_digit() {
            i += 1;
        }
        let width = fmt[w..i].parse().ok();
        let mut prec = None;
        if f.get(i) == Some(&b'.') {
            i += 1;
            let p = i;
            while i < f.len() && f[i].is_ascii_digit() {
                i += 1;
            }
            prec = Some(fmt[p..i].parse().unwrap_or(0));
        }
        let conv = f.get(i).copied().unwrap_or(b's');
        i += 1;
        let arg = args.next().copied().unwrap_or(Arg::Str("nil"));
        let sign = |neg: bool| {
            if neg {
                "-"
            } else if flags.contains('+') {
                "+"
            } else if flags.contains(' ') {
                " "
            } else {
                ""
            }
        };
        match (conv, arg) {
            (b's', a) => {
                let mut s = match a {
                    Arg::Num(n) => crate::pat::num(n),
                    Arg::Str(s) => s.to_string(),
                };
                if let Some(p) = prec {
                    s.truncate(p);
                }
                let fill = width.map_or(0, |w: usize| w.saturating_sub(s.len()));
                if flags.contains('-') {
                    out.push_str(&s);
                    out.push_str(&" ".repeat(fill));
                } else {
                    out.push_str(&" ".repeat(fill));
                    out.push_str(&s);
                }
            }
            (b'd' | b'i', a) => {
                let v = match a {
                    Arg::Num(n) => n as i64,
                    Arg::Str(s) => crate::pat::to_number(s).unwrap_or(0.0) as i64,
                };
                let mut b = v.unsigned_abs().to_string();
                if let Some(p) = prec
                    && b.len() < p
                {
                    b = "0".repeat(p - b.len()) + &b;
                }
                out.push_str(&pad(sign(v < 0), &b, width, flags, prec.is_none()));
            }
            (c, a) => {
                let x = match a {
                    Arg::Num(n) => n,
                    Arg::Str(s) => crate::pat::to_number(s).unwrap_or(0.0),
                };
                let p = prec.unwrap_or(6);
                let b = match c {
                    b'f' | b'F' => lua::format::format_f(x.abs(), p),
                    b'e' | b'E' => lua::format::format_e(x.abs(), p),
                    _ => lua::format::format_g_flags(x.abs(), p, flags.contains('#')),
                };
                let neg = x.is_sign_negative() && !x.is_nan();
                out.push_str(&pad(sign(neg), &b, width, flags, x.is_finite()));
            }
        }
    }
    out
}

/// A cell of a breakdown's table row: Lua's string, number or `false`, or
/// a tooltip function
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Str(String),
    Num(f64),
    Bool(bool),
    /// A function that fills a tooltip, of the item with this id
    ItemTooltip(f64),
}

impl From<f64> for Cell {
    fn from(n: f64) -> Self {
        Cell::Num(n)
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::Str(s)
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::Str(s.into())
    }
}

/// `x and "..." or false`
impl From<Option<String>> for Cell {
    fn from(s: Option<String>) -> Self {
        s.map_or(Cell::Bool(false), Cell::Str)
    }
}

/// A row of a breakdown's table, by column key
pub type Row = Vec<(&'static str, Cell)>;

/// A column of a breakdown's table (`colList`)
#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub label: String,
    pub key: &'static str,
    pub right: bool,
}

impl Col {
    pub fn new(label: impl Into<String>, key: &'static str) -> Col {
        Col {
            label: label.into(),
            key,
            right: false,
        }
    }
}

/// A row of a primary defence's table (`breakdown.slot`)
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub base: f64,
    pub inc: Option<String>,
    pub more: Option<String>,
    pub total: String,
    pub source: String,
    pub source_name: Option<String>,
    /// The item in the slot named by `source`, by id
    pub item: Option<f64>,
}

/// A row of a life or mana reservation's table
#[derive(Clone, Debug, PartialEq)]
pub struct Reservation {
    pub skill_name: String,
    pub base: Cell,
    pub mult: Option<String>,
    pub more: Option<String>,
    pub inc: Option<String>,
    pub efficiency: Option<String>,
    pub efficiency_more: Option<String>,
    pub total: Cell,
}

/// A buff's or debuff's mod with its value (`SkillBuffs`, `SkillDebuffs`)
#[derive(Clone, Debug, PartialEq)]
pub struct ModEntry {
    pub m: Mod,
    pub value: ModValue,
}

/// A breakdown: text lines and the tables the Calcs tab shows under them
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Breakdown {
    pub lines: Vec<String>,
    /// An area's radius, which the tab draws
    pub radius: Option<f64>,
    pub label: Option<String>,
    pub footer: Option<String>,
    /// Marks a table whose rows the tab sorts by value
    pub source: Option<String>,
    pub rows: Option<Vec<Row>>,
    pub cols: Option<Vec<Col>>,
    pub slots: Option<Vec<Slot>>,
    pub reservations: Option<Vec<Reservation>>,
    pub damage_types: Option<Vec<Row>>,
    pub mod_list: Option<Vec<ModEntry>>,
    /// The breakdown the Calcs tab shows for this one (`breakdownSource`)
    pub breakdown_source: Option<String>,
}

impl Breakdown {
    pub fn lines(lines: Vec<String>) -> Breakdown {
        Breakdown {
            lines,
            ..Breakdown::default()
        }
    }

    /// A table with these columns and no rows yet
    pub fn table(cols: Vec<Col>) -> Breakdown {
        Breakdown {
            rows: Some(Vec::new()),
            cols: Some(cols),
            ..Breakdown::default()
        }
    }

    pub fn push(&mut self, line: String) {
        self.lines.push(line);
    }

    pub fn push_row(&mut self, row: Row) {
        self.rows.get_or_insert_with(Vec::new).push(row);
    }
}

/// An actor's breakdowns by name; those of a weapon's pass are named
/// `MainHand.<name>` and `OffHand.<name>`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Breakdowns {
    pub map: HashMap<String, Breakdown>,
}

impl Breakdowns {
    pub fn get(&self, k: &str) -> Option<&Breakdown> {
        self.map.get(k)
    }

    pub fn set(&mut self, k: impl Into<String>, b: Breakdown) {
        self.map.insert(k.into(), b);
    }

    pub fn set_lines(&mut self, k: impl Into<String>, lines: Vec<String>) {
        self.set(k, Breakdown::lines(lines));
    }

    /// Sets or clears a breakdown (`breakdown.x = f(...)`, nil clears)
    pub fn set_opt(&mut self, k: impl Into<String>, lines: Option<Vec<String>>) {
        let k = k.into();
        match lines {
            Some(l) => self.set_lines(k, l),
            None => {
                self.map.remove(&k);
            }
        }
    }

    /// The breakdown, created empty if missing
    pub fn entry(&mut self, k: &str) -> &mut Breakdown {
        if !self.map.contains_key(k) {
            self.map.insert(k.to_string(), Breakdown::default());
        }
        self.map.get_mut(k).unwrap()
    }

    /// The breakdown, which an earlier step created (Lua fails on a
    /// missing one)
    pub fn existing(&mut self, k: &str) -> Result<&mut Breakdown> {
        self.map
            .get_mut(k)
            .ok_or_else(|| format!("attempt to index a nil value (field '{k}')"))
    }
}

/// A factor of [`multi_chain`]: a format with its value, `None` for nil
pub struct Factor<'a> {
    pub fmt: &'a str,
    pub value: Option<f64>,
    pub extra: Option<Arg<'a>>,
}

/// `{ fmt, value }`
pub fn f(fmt: &str, value: f64) -> Factor<'_> {
    Factor {
        fmt,
        value: Some(value),
        extra: None,
    }
}

/// `{ fmt, value, extra }`: a second format argument
pub fn f2<'a>(fmt: &'a str, value: f64, extra: impl Into<Arg<'a>>) -> Factor<'a> {
    Factor {
        fmt,
        value: Some(value),
        extra: Some(extra.into()),
    }
}

/// `{ fmt, value }` with a value that may be nil
pub fn fo(fmt: &str, value: Option<f64>) -> Factor<'_> {
    Factor {
        fmt,
        value,
        extra: None,
    }
}

fn factor_line(x: &Factor) -> String {
    let v = x.value.map_or(Arg::Str("nil"), Arg::Num);
    match x.extra {
        Some(e) => lua_format(x.fmt, &[v, e]),
        None => lua_format(x.fmt, &[v]),
    }
}

/// `breakdown.multiChain`'s chain
#[derive(Default)]
pub struct Chain<'a> {
    pub label: Option<&'a str>,
    pub base: Option<Factor<'a>>,
    pub factors: Vec<Factor<'a>>,
    pub total: Option<String>,
    pub no_total: bool,
}

/// `breakdown.multiChain(out, chain)`: the number of factors other than 1
pub fn multi_chain(out: &mut Vec<String>, chain: Chain) -> usize {
    let base = chain.base.as_ref().and_then(|b| b.value);
    let mut multiplier = 1.0;
    let mut lines = 0;
    if let Some(l) = chain.label {
        out.push(l.to_string());
    }
    if let Some(b) = chain.base.as_ref().filter(|b| b.value.is_some()) {
        out.push(factor_line(b));
    }
    if base != Some(0.0) {
        for m in &chain.factors {
            if let Some(v) = m.value
                && v != 1.0
            {
                multiplier *= v;
                out.push(format!("x {}", factor_line(m)));
                lines += 1;
            }
        }
    }
    if let Some(t) = chain.total {
        out.push(t);
    } else if (!chain.no_total && lines > 0 && base.is_some()) || (lines > 1 && base.is_none()) {
        out.push(sf!("= %.2f", multiplier * base.unwrap_or(1.0)));
    }
    lines
}

/// `breakdown.simple(extraBase, cfg, total, ...)` from the sums of the
/// names: `base`, `inc` and `more`
pub fn simple(extra_base: f64, base: f64, inc: f64, more: f64, total: f64) -> Option<Vec<String>> {
    if base + extra_base == 0.0
        || !(inc != 0.0 || more != 1.0 || (base != 0.0 && extra_base != 0.0))
    {
        return None;
    }
    let mut out = vec![if base != 0.0 && extra_base != 0.0 {
        sf!("(%g + %g) ^8(base)", extra_base, base)
    } else {
        sf!("%g ^8(base)", base + extra_base)
    }];
    if inc != 0.0 {
        out.push(sf!("x %.2f ^8(increased/reduced)", 1.0 + inc / 100.0));
    }
    if more != 1.0 {
        out.push(sf!("x %.2f ^8(more/less)", more));
    }
    out.push(sf!("= %g", total));
    Some(out)
}

/// `breakdown.mod(modList, cfg, ...)` from the sums
pub fn mod_(inc: f64, more: f64) -> Option<Vec<String>> {
    (inc != 0.0 || more != 1.0).then(|| {
        vec![
            sf!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
            sf!("x %.2f ^8(more/less)", more),
            sf!("= %.2f", (1.0 + inc / 100.0) * more),
        ]
    })
}

/// `breakdown.slot(source, sourceName, cfg, base, total, ...)` from the
/// sums, its row for the breakdown of the first name
pub fn slot(
    source: &str,
    source_name: Option<&str>,
    item: Option<f64>,
    base: f64,
    total: Option<f64>,
    inc: f64,
    more: f64,
) -> Slot {
    Slot {
        base,
        inc: (inc != 0.0).then(|| sf!(" x %.2f", 1.0 + inc / 100.0)),
        more: (more != 1.0).then(|| sf!(" x %.2f", more)),
        total: sf!("%.2f", total.unwrap_or(base * (1.0 + inc / 100.0) * more)),
        source: source.to_string(),
        source_name: source_name.map(String::from),
        item,
    }
}

/// `breakdown.area(base, areaMod, total, incBreakpoint, moreBreakpoint,
/// redBreakpoint, lessBreakpoint, label, radiusMod)`
pub fn area(
    base: f64,
    area_mod: f64,
    total: f64,
    breakpoints: Option<[f64; 4]>,
    label: Option<&str>,
    radius_mod: Option<f64>,
) -> Breakdown {
    let mut out = Vec::new();
    if let Some(l) = label {
        out.push(l.to_string());
    }
    if base != total {
        out.push(sf!("%.1fm ^8(base radius)", base / 10.0));
        out.push(sf!(
            "x %.2f ^8(square root of area of effect modifier)",
            (100.0 * area_mod.sqrt()).floor() / 100.0
        ));
        if let Some(m) = radius_mod.filter(|m| *m != 1.0) {
            out.push(sf!("x %.2f ^8(final radius modifier)", m));
        }
        out.push(sf!("= %.1fm", total / 10.0));
    }
    if let Some([inc, more, red, less]) = breakpoints {
        out.push(sf!(
            "^8Next 0.1m breakpoint: %d%% increased AoE / a %d%% more AoE multiplier",
            inc,
            more
        ));
        out.push(sf!(
            "^8Previous 0.1m breakpoint: %d%% reduced AoE / a %d%% less AoE multiplier",
            red,
            less
        ));
    }
    Breakdown {
        lines: out,
        radius: Some(total),
        ..Breakdown::default()
    }
}

/// The arguments of `breakdown.effMult`
pub struct EffMult<'a> {
    pub damage_type: &'a str,
    pub resist: f64,
    pub pen: f64,
    pub taken: f64,
    pub mult: f64,
    pub taken_more: f64,
    pub source_res: Option<&'a str>,
    pub use_res: bool,
    pub invert_chance: Option<f64>,
    pub effective_resist: Option<f64>,
}

/// `breakdown.effMult(...)`
pub fn eff_mult(a: EffMult) -> Vec<String> {
    let mut out = Vec::new();
    let form = if a.damage_type == "Physical" {
        "physical damage reduction"
    } else {
        "resistance"
    };
    let eff = a.effective_resist.unwrap_or(a.resist - a.pen);
    if let Some(src) = a.source_res.filter(|s| *s != a.damage_type) {
        out.push(sf!("Enemy %s: %d%% ^8(%s)", form, a.resist, src));
    } else if a.resist != 0.0 {
        out.push(sf!("Enemy %s: %d%%", form, a.resist));
    }
    match a.invert_chance {
        Some(c) if c != 0.0 && a.use_res => {
            out.push("Effective resistance:".into());
            out.push(sf!(
                "%g%% ^8(non-inverted hit after penetration)",
                a.resist - a.pen
            ));
            out.push(sf!(
                "%g%% ^8(inverted hit after penetration)",
                -a.resist - a.pen
            ));
            out.push(sf!(
                "= %g%% ^8(weighted average from %.0f%% inversion chance)",
                eff,
                c * 100.0
            ));
        }
        _ if a.pen != 0.0 || !a.use_res => {
            out.push("Effective resistance:".into());
            out.push(sf!("%d%% ^8(resistance)", a.resist));
            if a.pen < 0.0 {
                out.push(sf!("+ %d%% ^8(penetration)", -a.pen));
            } else if a.pen > 0.0 {
                out.push(sf!("- %d%% ^8(penetration)", a.pen));
            }
            if !a.use_res {
                out.push(sf!("x %d%% ^8(resistance ignored)", 0.0));
                out.push(sf!("= %d%%", 0.0));
            } else {
                out.push(sf!("= %d%%", a.resist - a.pen));
            }
        }
        _ => {}
    }
    if a.use_res {
        multi_chain(
            &mut out,
            Chain {
                label: Some("Effective DPS modifier:"),
                factors: vec![
                    f2("%.2f ^8(%s)", 1.0 - eff / 100.0, form),
                    f(
                        "%.2f ^8(increased/reduced damage taken)",
                        1.0 + a.taken / 100.0,
                    ),
                    f("%.2f ^8(more/less damage taken)", a.taken_more),
                ],
                total: Some(sf!("= %.3f", a.mult)),
                ..Chain::default()
            },
        );
    } else {
        out.push("Effective DPS modifier:".into());
        out.push(sf!("= %.3f ^8(increased/reduced damage taken)", a.mult));
    }
    out
}

/// `breakdown.dot(out, baseVal, inc, more, mult, rate, aura, effMult,
/// total)`
#[allow(clippy::too_many_arguments)]
pub fn dot(
    out: &mut Vec<String>,
    base: f64,
    inc: f64,
    more: f64,
    mult: Option<f64>,
    rate: Option<f64>,
    aura: Option<f64>,
    eff_mult: f64,
    total: f64,
) {
    multi_chain(
        out,
        Chain {
            base: Some(f("%.1f ^8(base damage per second)", base)),
            factors: vec![
                f("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                f("%.2f ^8(more/less)", more),
                f("%.2f ^8(multiplier)", 1.0 + mult.unwrap_or(0.0) / 100.0),
                fo("%.2f ^8(rate modifier)", rate),
                fo("%.3f ^8(aura effect modifier)", aura),
                f("%.3f ^8(effective DPS modifier)", eff_mult),
            ],
            total: Some(sf!("= %.1f ^8per second", total)),
            ..Chain::default()
        },
    );
}

/// `breakdown.critDot(dotMulti, critMulti, dotChance, critChance)`
pub fn crit_dot(dot_multi: f64, crit_multi: f64, dot_chance: f64, crit_chance: f64) -> Vec<String> {
    let mut out = Vec::new();
    if dot_chance > 0.0 {
        out.push("Contribution from Non-crits:".into());
        out.push(sf!("%.2f ^8(dot multiplier for non-crits)", dot_multi));
        out.push(sf!(
            "x %.4f ^8(portion of instances created by non-crits)",
            dot_chance
        ));
        out.push(sf!("= %.2f", dot_multi * dot_chance));
    }
    if crit_chance > 0.0 {
        out.push("Contribution from Crits:".into());
        out.push(sf!("%.2f ^8(dot multiplier for crits)", crit_multi));
        out.push(sf!(
            "x %.4f ^8(portion of instances created by crits)",
            crit_chance
        ));
        out.push(sf!("= %.2f", crit_multi * crit_chance));
    }
    if dot_chance > 0.0 && crit_chance > 0.0 && dot_multi != crit_multi {
        out.push("Effective DoT Multiplier:".into());
        out.push(sf!(
            "%.2f + %.2f",
            dot_multi * dot_chance,
            crit_multi * crit_chance
        ));
        out.push(sf!(
            "= %.2f",
            dot_multi * dot_chance + crit_multi * crit_chance
        ));
    }
    out
}
