//! `Modules/StatDescriber.lua`: a stat's description lines in a scope of
//! the stat description files (`Data/StatDescriptions`), as the Skills
//! tab's quality tooltip asks it of a gem's quality stats. The game data
//! keeps the descriptions of those stats only.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::breakdown::{Arg, lua_format};
use crate::pat;
use lua::Value;
use std::collections::{HashMap, HashSet};

/// A limit's bound: a number, any (`#`), or for the first bound "not the
/// second" (`!`)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bound {
    pub num: Option<f64>,
    pub not: bool,
}
crate::codec_struct!(Bound { num, not });

/// A description line: its limits, text, value functions and whether it
/// describes gem quality only
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desc {
    pub limit: Vec<(Bound, Bound)>,
    pub text: String,
    pub specials: Vec<(String, usize)>,
    pub gem_quality: bool,
}
crate::codec_struct!(Desc {
    limit,
    text,
    specials,
    gem_quality
});

/// The stats a description describes, and its lines (English)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Descriptor {
    pub stats: Vec<String>,
    pub lang: Option<Vec<Desc>>,
}
crate::codec_struct!(Descriptor { stats, lang });

/// A stat description file: its parent, and its descriptions by their
/// order with the stats' orders
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scope {
    pub parent: Option<String>,
    pub order: HashMap<String, u64>,
    pub descriptors: HashMap<u64, Descriptor>,
}
crate::codec_struct!(Scope {
    parent,
    order,
    descriptors
});

/// The scopes, by name
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatDescriptions {
    pub scopes: HashMap<String, Scope>,
}
crate::codec_struct!(StatDescriptions { scopes });

fn bound(v: &Value) -> Bound {
    match v {
        Value::Number(n) => Bound {
            num: Some(*n),
            not: false,
        },
        Value::Str(s) if s.to_str_lossy() == "!" => Bound {
            num: None,
            not: true,
        },
        _ => Bound::default(),
    }
}

impl StatDescriptions {
    /// The scopes from their files (`load(name)` gives a file's table),
    /// with the descriptions of `stats` only
    pub fn from_lua(
        names: &[String],
        load: &mut dyn FnMut(&str) -> Result<Value, String>,
        stats: &HashSet<String>,
    ) -> Result<StatDescriptions, String> {
        let mut out = StatDescriptions::default();
        let mut todo: Vec<String> = names.to_vec();
        while let Some(name) = todo.pop() {
            if out.scopes.contains_key(&name) {
                continue;
            }
            let t = load(&name)?;
            let t = t
                .as_table()
                .ok_or_else(|| format!("no stat descriptions {name}"))?;
            let parent = t
                .get_str("parent")
                .as_str()
                .map(|s| s.to_str_lossy().to_string());
            let mut scope = Scope {
                parent: parent.clone(),
                ..Scope::default()
            };
            for s in stats {
                let Value::Number(order) = t.get_str(s) else {
                    continue;
                };
                let order = order as u64;
                scope.order.insert(s.clone(), order);
                if scope.descriptors.contains_key(&order) {
                    continue;
                }
                let d = t.get(&Value::Number(order as f64));
                let d = d.as_table().ok_or("no stat description")?;
                let list = |v: Value| -> Vec<Value> {
                    v.as_table()
                        .map(|t| {
                            (1..)
                                .map_while(|i| match t.get(&Value::Number(i as f64)) {
                                    Value::Nil => None,
                                    v => Some(v),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let text = |v: &Value| {
                    v.as_str()
                        .map(|s| s.to_str_lossy().to_string())
                        .unwrap_or_default()
                };
                let stats: Vec<String> = list(d.get_str("stats")).iter().map(text).collect();
                let lang = match d.get(&Value::Number(1.0)) {
                    Value::Nil => None,
                    l => Some(
                        list(l)
                            .into_iter()
                            .filter_map(|e| {
                                let e = e.as_table()?;
                                let limit = list(e.get_str("limit"))
                                    .iter()
                                    .map(|l| {
                                        let l = l.as_table();
                                        let b = |i: f64| {
                                            l.map(|l| bound(&l.get(&Value::Number(i))))
                                                .unwrap_or_default()
                                        };
                                        (b(1.0), b(2.0))
                                    })
                                    .collect();
                                let specials = list(Value::Table(e.clone()))
                                    .iter()
                                    .filter_map(|s| {
                                        let s = s.as_table()?;
                                        let v = match s.get_str("v") {
                                            Value::Number(n) => n as usize,
                                            _ => 1,
                                        };
                                        Some((text(&s.get_str("k")), v))
                                    })
                                    .collect();
                                Some(Desc {
                                    limit,
                                    text: text(&e.get_str("text")),
                                    specials,
                                    gem_quality: e.get_str("gem_quality").truthy(),
                                })
                            })
                            .collect(),
                    ),
                };
                scope.descriptors.insert(order, Descriptor { stats, lang });
            }
            if let Some(p) = parent {
                todo.push(p);
            }
            out.scopes.insert(name, scope);
        }
        Ok(out)
    }

    /// The scope and its parents, nearest first (`scopeList`)
    fn chain(&self, name: &str) -> Vec<&Scope> {
        let mut out = Vec::new();
        let mut cur = self.scopes.get(name);
        while let Some(s) = cur {
            out.push(s);
            cur = s.parent.as_deref().and_then(|p| self.scopes.get(p));
        }
        out
    }

    /// `describeStats({ [stat] = value }, scope)`: the lines of one stat;
    /// `Err` for a value function the session cannot run
    pub fn describe(&self, stat: &str, value: f64, scope: &str) -> Result<Vec<String>, String> {
        if value == 0.0 {
            return Ok(vec![]);
        }
        let Some(d) = self
            .chain(scope)
            .into_iter()
            .find_map(|s| s.order.get(stat).and_then(|o| s.descriptors.get(o)))
        else {
            return Ok(vec![]);
        };
        let Some(lang) = &d.lang else {
            return Ok(vec![]);
        };
        // `val[i]`: min, max and format
        let mut val: Vec<(f64, f64, &'static str)> = d
            .stats
            .iter()
            .map(|s| {
                let v = if s == stat { value } else { 0.0 };
                (v, v, "d")
            })
            .collect();
        // `matchLimit(lang, val, nil)`
        let desc = lang.iter().filter(|e| !e.gem_quality).find(|e| {
            e.limit.iter().enumerate().all(|(i, (lo, hi))| {
                let min = val.get(i).map_or(0.0, |v| v.0);
                if lo.not {
                    return hi.num != Some(min);
                }
                !(hi.num.is_some_and(|h| min > h) || lo.num.is_some_and(|l| min < l))
            })
        });
        let Some(desc) = desc else {
            return Ok(vec![]);
        };
        for (k, v) in &desc.specials {
            apply_special(&mut val, k, *v)?;
        }
        let fmt = |v: &(f64, f64, &str), flag: &str| -> String {
            let one = |x: f64| lua_format(&format!("%{flag}{}", v.2), &[Arg::Num(x)]);
            if v.0 == v.1 {
                one(v.0)
            } else {
                format!("({}-{})", one(v.0), one(v.1))
            }
        };
        let at = |n: &str| -> Option<&(f64, f64, &str)> { val.get(n.parse::<usize>().ok()?) };
        let mut text = pat::gsub_fn(&desc.text, "{(%d)}", |c| at(&c[0]).map(|v| fmt(v, "")));
        text = pat::gsub_fn(&text, "{}", |_| val.first().map(|v| fmt(v, "")));
        text = pat::gsub_fn(&text, "{:%+?d}", |_| val.first().map(|v| fmt(v, "")));
        text = pat::gsub_fn(&text, "{(%d):(%+?)d}", |c| {
            let v = at(&c[0])?;
            let (min, max) = (&v.0, &v.1);
            if min == max || c[1] != "+" {
                Some(fmt(v, &c[1]))
            } else if *max < 0.0 {
                Some(lua_format("-(%d-%d)", &[Arg::Num(-min), Arg::Num(-max)]))
            } else {
                Some(lua_format("+(%d-%d)", &[Arg::Num(*min), Arg::Num(*max)]))
            }
        });
        text = pat::gsub(&text, "%%%%", "%%");
        Ok(pat::gmatch(&format!("{text}\\n"), "([^\\]+)\\n")
            .into_iter()
            .filter_map(|c| c.into_iter().next())
            .collect())
    }
}

/// `round(x, n)` of `Modules/Common.lua`
fn round(x: f64, n: i32) -> f64 {
    crate::calc::round(x, n as f64)
}

/// `applySpecial(val, spec)`
fn apply_special(val: &mut [(f64, f64, &'static str)], k: &str, v: usize) -> Result<(), String> {
    let Some(e) = val.get_mut(v.wrapping_sub(1)) else {
        return Ok(());
    };
    let (min, max) = (e.0, e.1);
    let mut set = |a: f64, b: f64, g: bool| {
        e.0 = a;
        e.1 = b;
        if g {
            e.2 = "g";
        }
    };
    let both = |f: &dyn Fn(f64) -> f64| (f(min), f(max));
    match k {
        "negate" => set(-max, -min, false),
        "invert_chance" => set(100.0 - max, 100.0 - min, false),
        "negate_and_double" => set(-2.0 * max, -2.0 * min, false),
        "divide_by_two_0dp" => {
            let (a, b) = both(&|x| x / 2.0);
            set(a, b, false)
        }
        "divide_by_ten_0dp" => {
            let (a, b) = both(&|x| x / 10.0);
            set(a, b, false)
        }
        "divide_by_fifteen_0dp" => {
            let (a, b) = both(&|x| x / 15.0);
            set(a, b, false)
        }
        "divide_by_four" => {
            let (a, b) = both(&|x| x / 4.0);
            set(a, b, true)
        }
        "divide_by_five" => {
            let (a, b) = both(&|x| x / 5.0);
            set(a, b, true)
        }
        "divide_by_six" => {
            let (a, b) = both(&|x| x / 6.0);
            set(a, b, true)
        }
        "divide_by_ten_1dp_if_required" | "divide_by_ten_1dp" => {
            let (a, b) = both(&|x| round(x / 10.0, 1));
            set(a, b, true)
        }
        "divide_by_twelve" => {
            let (a, b) = both(&|x| x / 12.0);
            set(a, b, true)
        }
        "divide_by_twenty" => {
            let (a, b) = both(&|x| x / 20.0);
            set(a, b, true)
        }
        "divide_by_one_hundred" => {
            let (a, b) = both(&|x| x / 100.0);
            set(a, b, true)
        }
        "divide_by_one_hundred_1dp" => {
            let (a, b) = both(&|x| round(x / 100.0, 1));
            set(a, b, true)
        }
        "divide_by_one_hundred_2dp_if_required" | "divide_by_one_hundred_2dp" => {
            let (a, b) = both(&|x| round(x / 100.0, 2));
            set(a, b, true)
        }
        "divide_by_one_hundred_and_negate" => {
            let (a, b) = both(&|x| -round(x / 100.0, 1));
            set(a, b, true)
        }
        "divide_by_twenty_then_double_0dp" => {
            let (a, b) = both(&|x| round(x / 20.0, 0) * 2.0);
            set(a, b, false)
        }
        "divide_by_one_thousand" => {
            let (a, b) = both(&|x| x / 1000.0);
            set(a, b, true)
        }
        "per_minute_to_per_second" | "per_minute_to_per_second_1dp" => {
            let (a, b) = both(&|x| round(x / 60.0, 1));
            set(a, b, true)
        }
        "permyriad_per_minute_to_%_per_second" => {
            let (a, b) = both(&|x| round(x / 60.0 / 100.0, 1));
            set(a, b, true)
        }
        "per_minute_to_per_second_0dp" => {
            let (a, b) = both(&|x| x / 60.0);
            set(a, b, false)
        }
        "per_minute_to_per_second_2dp" | "per_minute_to_per_second_2dp_if_required" => {
            let (a, b) = both(&|x| round(x / 60.0, 2));
            set(a, b, true)
        }
        "milliseconds_to_seconds" => {
            let (a, b) = both(&|x| x / 1000.0);
            set(a, b, true)
        }
        "milliseconds_to_seconds_0dp" => {
            let (a, b) = both(&|x| x / 1000.0);
            set(a, b, false)
        }
        "milliseconds_to_seconds_1dp" => {
            let (a, b) = both(&|x| round(x / 1000.0, 1));
            set(a, b, true)
        }
        "milliseconds_to_seconds_2dp" | "milliseconds_to_seconds_2dp_if_required" => {
            let (a, b) = both(&|x| round(x / 1000.0, 2));
            set(a, b, true)
        }
        "deciseconds_to_seconds" | "locations_to_metres" => {
            let (a, b) = both(&|x| x / 10.0);
            set(a, b, true)
        }
        "30%_of_value" => {
            let (a, b) = both(&|x| x * 0.3);
            set(a, b, false)
        }
        "60%_of_value" => {
            let (a, b) = both(&|x| x * 0.6);
            set(a, b, false)
        }
        "multiplicative_damage_modifier" => {
            let (a, b) = both(&|x| 100.0 + x);
            set(a, b, false)
        }
        "multiplicative_permyriad_damage_modifier" => {
            let (a, b) = both(&|x| 100.0 + round(x / 100.0, 1));
            set(a, b, true)
        }
        "multiply_by_four" => {
            let (a, b) = both(&|x| x * 4.0);
            set(a, b, false)
        }
        "times_one_point_five" => {
            let (a, b) = both(&|x| x * 1.5);
            set(a, b, false)
        }
        "times_twenty" => {
            let (a, b) = both(&|x| x * 20.0);
            set(a, b, false)
        }
        "double" => {
            let (a, b) = both(&|x| x * 2.0);
            set(a, b, false)
        }
        "plus_two_hundred" => {
            let (a, b) = both(&|x| x + 200.0);
            set(a, b, false)
        }
        "display_indexable_non_active_support" => {
            return Err("a support's name by its index (Data.PearlSupports)".into());
        }
        // `reminderstring`, `canonical_line`, ...: nothing; an unknown
        // function only prints a message
        _ => {}
    }
    Ok(())
}
