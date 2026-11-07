//! Calculation breakdowns: the tables the Calcs tab shows for an output
//! (`actor.breakdown`, only built in `CALCS` mode), and upstream's
//! breakdown generators (`Modules/CalcBreakdown.lua`).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! A breakdown is a Lua table: lines of text (its array part) and named
//! fields (`slots`, `rowList`, `radius`, ...). [`Bd`] keeps both.

use crate::rt;
use lua::Value;
use std::collections::BTreeMap;
use std::rc::Rc;

/// A value in a breakdown table
#[derive(Clone, Debug, PartialEq)]
pub enum BdVal {
    Str(Rc<str>),
    Num(f64),
    Bool(bool),
    Table(Bd),
    /// An item, by name (`item = actor.itemList[slot]`)
    Item(Rc<str>),
}

impl From<&str> for BdVal {
    fn from(s: &str) -> BdVal {
        BdVal::Str(s.into())
    }
}

impl From<String> for BdVal {
    fn from(s: String) -> BdVal {
        BdVal::Str(s.into())
    }
}

impl From<f64> for BdVal {
    fn from(n: f64) -> BdVal {
        BdVal::Num(n)
    }
}

impl From<bool> for BdVal {
    fn from(b: bool) -> BdVal {
        BdVal::Bool(b)
    }
}

impl From<Bd> for BdVal {
    fn from(b: Bd) -> BdVal {
        BdVal::Table(b)
    }
}

/// A breakdown table: its lines and its named fields
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bd {
    pub lines: Vec<BdVal>,
    pub fields: BTreeMap<String, BdVal>,
}

impl Bd {
    pub fn new() -> Bd {
        Bd::default()
    }

    /// A table of these lines
    pub fn of<I: IntoIterator<Item = S>, S: Into<BdVal>>(lines: I) -> Bd {
        Bd {
            lines: lines.into_iter().map(Into::into).collect(),
            fields: BTreeMap::new(),
        }
    }

    /// `t_insert(t, v)`
    pub fn push(&mut self, v: impl Into<BdVal>) {
        self.lines.push(v.into());
    }

    /// `t[k] = v`
    pub fn set(&mut self, k: &str, v: impl Into<BdVal>) {
        self.fields.insert(k.to_string(), v.into());
    }

    pub fn get(&self, k: &str) -> Option<&BdVal> {
        self.fields.get(k)
    }

    /// The table in field `k`, created when missing (`t[k] = t[k] or {}`)
    pub fn table(&mut self, k: &str) -> &mut Bd {
        let e = self
            .fields
            .entry(k.to_string())
            .or_insert_with(|| BdVal::Table(Bd::new()));
        if !matches!(e, BdVal::Table(_)) {
            *e = BdVal::Table(Bd::new());
        }
        match e {
            BdVal::Table(t) => t,
            _ => unreachable!(),
        }
    }

    /// The table in field `k`, if it is one
    pub fn table_mut(&mut self, k: &str) -> Option<&mut Bd> {
        match self.fields.get_mut(k) {
            Some(BdVal::Table(t)) => Some(t),
            _ => None,
        }
    }
}

/// An argument of [`sf`]
#[derive(Clone, Debug)]
pub enum Arg {
    N(f64),
    S(Rc<str>),
    Nil,
}

impl From<f64> for Arg {
    fn from(n: f64) -> Arg {
        Arg::N(n)
    }
}

impl From<&str> for Arg {
    fn from(s: &str) -> Arg {
        Arg::S(s.into())
    }
}

impl From<String> for Arg {
    fn from(s: String) -> Arg {
        Arg::S(s.into())
    }
}

impl From<&String> for Arg {
    fn from(s: &String) -> Arg {
        Arg::S(s.as_str().into())
    }
}

impl From<Rc<str>> for Arg {
    fn from(s: Rc<str>) -> Arg {
        Arg::S(s)
    }
}

impl From<Option<f64>> for Arg {
    fn from(n: Option<f64>) -> Arg {
        n.map_or(Arg::Nil, Arg::N)
    }
}

/// `string.format(f, ...)`
pub fn sf(f: &str, args: &[Arg]) -> String {
    let args: Vec<Value> = args
        .iter()
        .map(|a| match a {
            Arg::N(n) => Value::Number(*n),
            Arg::S(s) => Value::str(s),
            Arg::Nil => Value::Nil,
        })
        .collect();
    lua::to_string(&rt::format(&Value::str(f), &args)).to_string()
}

/// `s_format(f, ...)` with arguments of mixed types
#[macro_export]
macro_rules! sf {
    ($f:expr $(, $a:expr)* $(,)?) => {
        $crate::breakdown::sf($f, &[$($crate::breakdown::Arg::from($a)),*])
    };
}

/// One factor of a [`multi_chain`]: its format and arguments, the first
/// of which is its value
pub struct Factor {
    pub fmt: &'static str,
    pub args: Vec<Arg>,
}

impl Factor {
    pub fn new(fmt: &'static str, args: Vec<Arg>) -> Factor {
        Factor { fmt, args }
    }

    fn value(&self) -> Option<f64> {
        match self.args.first() {
            Some(Arg::N(n)) => Some(*n),
            _ => None,
        }
    }
}

/// A factor with a value and optional further arguments
#[macro_export]
macro_rules! factor {
    ($f:expr $(, $a:expr)* $(,)?) => {
        $crate::breakdown::Factor::new($f, vec![$($crate::breakdown::Arg::from($a)),*])
    };
}

/// The chain `breakdown.multiChain` writes
#[derive(Default)]
pub struct Chain {
    pub label: Option<String>,
    pub base: Option<Factor>,
    pub factors: Vec<Factor>,
    pub total: Option<String>,
    pub no_total: bool,
}

/// `breakdown.multiChain(out, chain)`: the number of factors written
pub fn multi_chain(out: &mut Bd, chain: Chain) -> usize {
    let base = chain.base.as_ref().and_then(Factor::value);
    let mut multiplier = 1.0;
    let mut lines = 0;
    if let Some(l) = chain.label {
        out.push(l);
    }
    if let Some(b) = &chain.base
        && base.is_some()
    {
        out.push(sf(b.fmt, &b.args));
    }
    if base != Some(0.0) {
        for f in &chain.factors {
            if let Some(v) = f.value()
                && v != 1.0
            {
                multiplier *= v;
                out.push(format!("x {}", sf(f.fmt, &f.args)));
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

/// `breakdown.mod(modList, cfg, ...)` from the sums it reads
pub fn mod_(inc: f64, more: f64) -> Option<Bd> {
    if inc != 0.0 || more != 1.0 {
        Some(Bd::of([
            sf!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
            sf!("x %.2f ^8(more/less)", more),
            sf!("= %.2f", (1.0 + inc / 100.0) * more),
        ]))
    } else {
        None
    }
}

/// `breakdown.simple(extraBase, cfg, total, ...)` from the sums it reads
pub fn simple(extra_base: f64, base: f64, inc: f64, more: f64, total: f64) -> Option<Bd> {
    if base + extra_base == 0.0 {
        return None;
    }
    if !(inc != 0.0 || more != 1.0 || (base != 0.0 && extra_base != 0.0)) {
        return None;
    }
    let mut out = Bd::new();
    if base != 0.0 && extra_base != 0.0 {
        out.push(sf!("(%g + %g) ^8(base)", extra_base, base));
    } else {
        out.push(sf!("%g ^8(base)", base + extra_base));
    }
    if inc != 0.0 {
        out.push(sf!("x %.2f ^8(increased/reduced)", 1.0 + inc / 100.0));
    }
    if more != 1.0 {
        out.push(sf!("x %.2f ^8(more/less)", more));
    }
    out.push(sf!("= %g", total));
    Some(out)
}

/// `breakdown.area(base, areaMod, total, ...)`
#[allow(clippy::too_many_arguments)]
pub fn area(
    base: f64,
    area_mod: f64,
    total: f64,
    breakpoints: Option<[f64; 4]>,
    label: Option<&str>,
) -> Bd {
    let mut out = Bd::new();
    if let Some(l) = label {
        out.push(l);
    }
    if base != total {
        out.push(sf!("%.1fm ^8(base radius)", base / 10.0));
        out.push(sf!(
            "x %.2f ^8(square root of area of effect modifier)",
            (100.0 * area_mod.sqrt()).floor() / 100.0
        ));
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
    out.set("radius", total);
    out
}

/// `breakdown.effMult(damageType, resist, pen, taken, mult, takenMore,
/// sourceRes, useRes, invertChance, effectiveResist)`
#[allow(clippy::too_many_arguments)]
pub fn eff_mult(
    damage_type: &str,
    resist: f64,
    pen: f64,
    taken: f64,
    mult: f64,
    taken_more: f64,
    source_res: Option<&str>,
    use_res: bool,
    invert_chance: Option<f64>,
    effective_resist: Option<f64>,
) -> Bd {
    let mut out = Bd::new();
    let resist_form = if damage_type == "Physical" {
        "physical damage reduction"
    } else {
        "resistance"
    };
    let effective = effective_resist.unwrap_or(resist - pen);
    match source_res {
        Some(s) if s != damage_type => {
            out.push(sf!("Enemy %s: %d%% ^8(%s)", resist_form, resist, s));
        }
        _ if resist != 0.0 => out.push(sf!("Enemy %s: %d%%", resist_form, resist)),
        _ => {}
    }
    let invert = invert_chance.filter(|&c| c != 0.0);
    if let (Some(chance), true) = (invert, use_res) {
        out.push("Effective resistance:");
        out.push(sf!(
            "%g%% ^8(non-inverted hit after penetration)",
            resist - pen
        ));
        out.push(sf!(
            "%g%% ^8(inverted hit after penetration)",
            -resist - pen
        ));
        out.push(sf!(
            "= %g%% ^8(weighted average from %.0f%% inversion chance)",
            effective,
            chance * 100.0
        ));
    } else if pen != 0.0 || !use_res {
        out.push("Effective resistance:");
        out.push(sf!("%d%% ^8(resistance)", resist));
        if pen < 0.0 {
            out.push(sf!("+ %d%% ^8(penetration)", -pen));
        } else if pen > 0.0 {
            out.push(sf!("- %d%% ^8(penetration)", pen));
        }
        if !use_res {
            out.push(sf!("x %d%% ^8(resistance ignored)", 0.0));
            out.push(sf!("= %d%%", 0.0));
        } else {
            out.push(sf!("= %d%%", resist - pen));
        }
    }
    if use_res {
        multi_chain(
            &mut out,
            Chain {
                label: Some("Effective DPS modifier:".into()),
                factors: vec![
                    factor!("%.2f ^8(%s)", 1.0 - effective / 100.0, resist_form),
                    factor!(
                        "%.2f ^8(increased/reduced damage taken)",
                        1.0 + taken / 100.0
                    ),
                    factor!("%.2f ^8(more/less damage taken)", taken_more),
                ],
                total: Some(sf!("= %.3f", mult)),
                ..Chain::default()
            },
        );
    } else {
        out.push("Effective DPS modifier:");
        out.push(sf!("= %.3f ^8(increased/reduced damage taken)", mult));
    }
    out
}

/// `breakdown.dot(out, baseVal, inc, more, mult, rate, aura, effMult,
/// total)`
#[allow(clippy::too_many_arguments)]
pub fn dot(
    out: &mut Bd,
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
            base: Some(factor!("%.1f ^8(base damage per second)", base)),
            factors: vec![
                factor!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                factor!("%.2f ^8(more/less)", more),
                factor!("%.2f ^8(multiplier)", 1.0 + mult.unwrap_or(0.0) / 100.0),
                factor!("%.2f ^8(rate modifier)", rate),
                factor!("%.3f ^8(aura effect modifier)", aura),
                factor!("%.3f ^8(effective DPS modifier)", eff_mult),
            ],
            total: Some(sf!("= %.1f ^8per second", total)),
            ..Chain::default()
        },
    );
}

/// `breakdown.critDot(dotMulti, critMulti, dotChance, critChance)`
pub fn crit_dot(dot_multi: f64, crit_multi: f64, dot_chance: f64, crit_chance: f64) -> Bd {
    let combined = dot_multi * dot_chance + crit_multi * crit_chance;
    let mut out = Bd::new();
    if dot_chance > 0.0 {
        out.push("Contribution from Non-crits:");
        out.push(sf!("%.2f ^8(dot multiplier for non-crits)", dot_multi));
        out.push(sf!(
            "x %.4f ^8(portion of instances created by non-crits)",
            dot_chance
        ));
        out.push(sf!("= %.2f", dot_multi * dot_chance));
    }
    if crit_chance > 0.0 {
        out.push("Contribution from Crits:");
        out.push(sf!("%.2f ^8(dot multiplier for crits)", crit_multi));
        out.push(sf!(
            "x %.4f ^8(portion of instances created by crits)",
            crit_chance
        ));
        out.push(sf!("= %.2f", crit_multi * crit_chance));
    }
    if dot_chance > 0.0 && crit_chance > 0.0 && dot_multi != crit_multi {
        out.push("Effective DoT Multiplier:");
        out.push(sf!(
            "%.2f + %.2f",
            dot_multi * dot_chance,
            crit_multi * crit_chance
        ));
        out.push(sf!("= %.2f", combined));
    }
    out
}

/// A breakdown shown as a table: `{ label = ..., rowList = { }, colList =
/// { { label = ..., key = ... }, ... } }`
pub fn row_table(label: &str, cols: &[(&str, &str)]) -> Bd {
    let mut t = Bd::new();
    t.set("label", label);
    t.set("rowList", Bd::new());
    let mut list = Bd::new();
    for (l, k) in cols {
        let mut c = Bd::new();
        c.set("label", *l);
        c.set("key", *k);
        list.push(c);
    }
    t.set("colList", list);
    t
}

/// A row of a [`row_table`]: its cells by key
pub fn row<V: Into<BdVal>>(cells: impl IntoIterator<Item = (&'static str, V)>) -> Bd {
    let mut t = Bd::new();
    for (k, v) in cells {
        t.set(k, v);
    }
    t
}

impl Bd {
    /// Adds a row to `rowList`
    pub fn add_row(&mut self, row: Bd) {
        self.table("rowList").push(row);
    }
}
