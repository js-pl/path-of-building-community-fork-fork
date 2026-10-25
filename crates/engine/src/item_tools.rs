//! Item mod line helpers: upstream's `itemLib` (`Modules/ItemTools.lua`),
//! which rolls a line's ranges and scales its values.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::item_data::ItemData;
use crate::mod_parser::ModParser;
use crate::mods::convert::get;
use crate::pat::{self, gsub_fn, gsub_fn_n, is_match, num, to_number};
use lua::Value;

pub type Result<T> = std::result::Result<T, String>;

/// Where a line's ranges roll: one value for all, or one per range
#[derive(Clone, Debug, PartialEq)]
pub enum Range {
    One(f64),
    Each(Vec<f64>),
}

/// `round(val, dec)` of `Modules/Common.lua`
pub fn round(val: f64, dec: Option<f64>) -> f64 {
    match dec {
        Some(d) => (val * 10f64.powf(d) + 0.5).floor() / 10f64.powf(d),
        None => (val + 0.5).floor(),
    }
}

/// `roundSymmetric(val, dec)`: halves round away from zero
pub fn round_symmetric(val: f64, dec: Option<f64>) -> f64 {
    let factor = dec.map(|d| 10f64.powf(d));
    let v = factor.map_or(val, |f| val * f);
    let r = if val >= 0.0 {
        (v + 0.5).floor()
    } else {
        (v - 0.5).ceil()
    };
    factor.map_or(r, |f| r / f)
}

/// `floorSymmetric(val)`: towards zero
fn floor_symmetric(val: f64) -> f64 {
    val.trunc()
}

/// `alwaysPositiveRound(val)`
fn always_positive_round(val: f64) -> f64 {
    floor_symmetric(val + 0.5)
}

fn tonum(s: &str) -> Result<f64> {
    to_number(s).ok_or_else(|| format!("attempt to perform arithmetic on a string value ({s:?})"))
}

fn antonym(caps: &[String]) -> Option<String> {
    let (n, word) = (&caps[0], &caps[1]);
    let anto = match word.as_str() {
        "increased" => Some("reduced"),
        "reduced" => Some("increased"),
        "more" => Some("less"),
        "less" => Some("more"),
        _ => None,
    };
    Some(match anto {
        Some(a) => format!("{n} {a}"),
        None => format!("-{n} {word}"),
    })
}

/// `itemLib.applyValueScalar`: scales the first `numbers` numbers of a line
pub fn apply_value_scalar(
    line: &str,
    value_scalar: Option<f64>,
    base_value_scalar: Option<f64>,
    numbers: usize,
    precision: Option<f64>,
) -> Result<String> {
    let vs = value_scalar.unwrap_or(1.0);
    if vs == 1.0 && base_value_scalar.is_none_or(|b| b == 1.0) {
        return Ok(line.to_string());
    }
    let power = 10f64.powf(precision.unwrap_or(0.0));
    let mut err = None;
    let mut scale = |n: &str| -> String {
        let mut value = match tonum(n) {
            Ok(v) => v,
            Err(e) => {
                err = Some(e);
                return String::new();
            }
        };
        if let Some(b) = base_value_scalar.filter(|&b| b != 1.0) {
            value = round(value * b * power, None) / power;
        }
        let bias = if precision.is_some() { 0.0 } else { 0.001 };
        num((value * vs * power + bias).floor() / power)
    };
    let out = if precision.is_some() {
        gsub_fn_n(line, "(%d+%.?%d*)", Some(numbers), |c| Some(scale(&c[0]))).0
    } else {
        gsub_fn_n(line, "(%d+)([^%.])", Some(numbers), |c| {
            Some(scale(&c[0]) + &c[1])
        })
        .0
    };
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// `itemLib.formatValue`
fn format_value(
    value: f64,
    base_value_scalar: Option<f64>,
    value_scalar: Option<f64>,
    precision: f64,
    display_precision: Option<f64>,
    if_required: bool,
) -> String {
    let mut value = round_symmetric(value * precision, None);
    if let Some(b) = base_value_scalar.filter(|&b| b != 1.0) {
        value = always_positive_round(value * b);
    }
    if let Some(v) = value_scalar.filter(|&v| v != 1.0) {
        value = floor_symmetric(value * v);
    }
    value /= precision;
    if let Some(dp) = display_precision {
        value = round_symmetric(value, Some(dp));
    }
    match display_precision {
        Some(dp) if !if_required => format!("{:.*}", dp as usize, value),
        Some(_) => num(value),
        None => {
            let dec = (precision.ln() / 10f64.ln() + 0.001).floor().min(2.0);
            num(round_symmetric(value, Some(dec)))
        }
    }
}

/// `itemLib.isZeroValueLine`
pub fn is_zero_value_line(line: &str) -> bool {
    is_match(line, "^%+?0%%? ")
        || (is_match(line, " %+?0%%? ")
            && !is_match(line, "0 to [1-9]")
            && !is_match(line, "0%% to %d+%%"))
        || is_match(line, " 0%-0 ")
        || is_match(line, " 0 to 0 ")
}

/// The scalability formats as `(precision, displayPrecision, ifRequired)`
fn format_precision(formats: &[String]) -> (Option<f64>, Option<f64>, bool) {
    let (mut p, mut dp, mut req) = (None, None, false);
    for f in formats {
        match f.as_str() {
            "divide_by_two_0dp" => (p, dp, req) = (Some(2.0), Some(0.0), true),
            "divide_by_three" => p = Some(3.0),
            "divide_by_four" => p = Some(4.0),
            "divide_by_five" => p = Some(5.0),
            "divide_by_six" => p = Some(6.0),
            "divide_by_ten_0dp" => (p, dp) = (Some(10.0), Some(0.0)),
            "divide_by_ten_1dp" => (p, dp) = (Some(10.0), Some(1.0)),
            "divide_by_ten_1dp_if_required" => (p, dp, req) = (Some(10.0), Some(1.0), true),
            "divide_by_twelve" => p = Some(12.0),
            "divide_by_fifteen_0dp" => (p, dp) = (Some(15.0), Some(0.0)),
            "divide_by_twenty" => p = Some(20.0),
            "divide_by_twenty_then_double_0dp" => (p, dp) = (Some(10.0), Some(0.0)),
            "divide_by_one_hundred" | "divide_by_one_hundred_and_negate" => p = Some(100.0),
            "divide_by_one_hundred_0dp" => (p, dp) = (Some(100.0), Some(0.0)),
            "divide_by_one_hundred_1dp" => (p, dp) = (Some(100.0), Some(1.0)),
            "divide_by_one_hundred_2dp" => (p, dp) = (Some(100.0), Some(2.0)),
            "divide_by_one_hundred_2dp_if_required" => {
                (p, dp, req) = (Some(100.0), Some(2.0), true)
            }
            "divide_by_one_thousand" => p = Some(1000.0),
            "divide_by_ten_thousand_1dp" => (p, dp) = (Some(10000.0), Some(1.0)),
            "per_minute_to_per_second" => p = Some(60.0),
            "per_minute_to_per_second_0dp" => (p, dp) = (Some(60.0), Some(0.0)),
            "per_minute_to_per_second_1dp" => (p, dp) = (Some(60.0), Some(1.0)),
            "per_minute_to_per_second_2dp" => (p, dp) = (Some(60.0), Some(2.0)),
            "per_minute_to_per_second_2dp_if_required" => {
                (p, dp, req) = (Some(60.0), Some(2.0), true)
            }
            "milliseconds_to_seconds" | "milliseconds_to_seconds_halved" => p = Some(1000.0),
            "milliseconds_to_seconds_0dp" => (p, dp) = (Some(1000.0), Some(0.0)),
            "milliseconds_to_seconds_1dp" => (p, dp) = (Some(1000.0), Some(1.0)),
            "milliseconds_to_seconds_2dp" => (p, dp) = (Some(1000.0), Some(2.0)),
            "locations_to_metres" => (p, dp) = (Some(10.0), Some(1.0)),
            "milliseconds_to_seconds_2dp_if_required" => {
                (p, dp, req) = (Some(1000.0), Some(2.0), true)
            }
            "deciseconds_to_seconds" => p = Some(10.0),
            _ => {}
        }
    }
    (p, dp, req)
}

/// `findScalableLine`: substitutes values back into a stripped line, most
/// first, until a line with scalability data is found; gives the line and
/// the values left as `#`.
fn find_scalable_line(
    data: &ItemData,
    line: &str,
    values: &[String],
) -> Option<(String, Vec<String>)> {
    let key = |l: &str| pat::gsub(l, "+#", "#");
    fn replace_nth(input: &str, replacement: &str, n: usize) -> String {
        let mut count = 0;
        gsub_fn(input, "#", |m| {
            count += 1;
            Some(if count == n {
                replacement.to_string()
            } else {
                m[0].clone()
            })
        })
    }
    fn combos(
        data: &ItemData,
        line: &str,
        values: &[String],
        i: usize,
        num: usize,
        indices: &mut Vec<usize>,
        key: &dyn Fn(&str) -> String,
    ) -> Option<(String, Vec<String>)> {
        if indices.len() == num {
            let mut modified = line.to_string();
            for (substituted, &i) in indices.iter().enumerate() {
                modified = replace_nth(&modified, &values[i - 1], i - substituted);
            }
            if data.mod_scalability.contains_key(&key(&modified)) {
                let remaining = values
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| !indices.contains(&(j + 1)))
                    .map(|(_, v)| v.clone())
                    .collect();
                return Some((modified, remaining));
            }
            return None;
        }
        for j in i..=values.len() {
            indices.push(j);
            if let Some(found) = combos(data, line, values, j + 1, num, indices, key) {
                return Some(found);
            }
            indices.pop();
        }
        None
    }
    for n in (1..=values.len()).rev() {
        if let Some(found) = combos(data, line, values, 1, n, &mut Vec::new(), &key) {
            return Some(found);
        }
    }
    data.mod_scalability
        .contains_key(&key(line))
        .then(|| (line.to_string(), values.to_vec()))
}

/// `itemLib.applyRange`: rolls the line's ranges (`(x-y)`) at `range`
/// (0 to 1) and scales its values by the mod magnitude (`valueScalar`) and
/// the corruption multiplier (`baseValueScalar`).
pub fn apply_range(
    data: &ItemData,
    parser: &ModParser,
    line: &str,
    range: &Range,
    value_scalar: Option<f64>,
    base_value_scalar: Option<f64>,
) -> Result<String> {
    let mut err = None;
    let mut range_index = 0;
    let stripped = gsub_fn(line, "([%+-]?)%((%-?%d+%.?%d*)%-(%-?%d+%.?%d*)%)", |c| {
        range_index += 1;
        let r = match range {
            Range::Each(list) => list.get(range_index - 1).copied().unwrap_or(0.5),
            Range::One(r) => *r,
        };
        let (min, max) = match (tonum(&c[1]), tonum(&c[2])) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                err = Some(e);
                return None;
            }
        };
        let mut value = min + r * (max - min);
        if c[0] == "-" {
            value *= -1.0;
        }
        Some(if c[0] == "+" && value > 0.0 {
            format!("+{}", num(value))
        } else {
            num(value)
        })
    });
    if let Some(e) = err {
        return Err(e);
    }
    let stripped = gsub_fn(&stripped, "%-(%d+%.?%d*%%) (%a+)", antonym);
    let mut values = Vec::new();
    let stripped = gsub_fn(&stripped, "(%-?%d+%.?%d*)", |c| {
        values.push(c[0].clone());
        Some("#".into())
    });

    if let Some((mut scalable_line, mut scalable_values)) =
        find_scalable_line(data, &stripped, &values)
    {
        let key = pat::gsub(&scalable_line, "+#", "#");
        for (i, scalability) in data.mod_scalability[&key].iter().enumerate() {
            let (precision, dp, if_required) = format_precision(&scalability.formats);
            let v = scalable_values
                .get(i)
                .ok_or("attempt to perform arithmetic on a nil value")?;
            let v = tonum(v)?;
            let scaled = scalability.is_scalable
                && (base_value_scalar.is_some_and(|b| b != 1.0)
                    || value_scalar.is_some_and(|v| v != 1.0));
            scalable_values[i] = if scaled {
                format_value(
                    v,
                    base_value_scalar,
                    value_scalar,
                    precision.unwrap_or(1.0),
                    dp,
                    if_required,
                )
            } else {
                format_value(
                    v,
                    Some(1.0),
                    Some(1.0),
                    precision.unwrap_or(1.0),
                    dp,
                    if_required,
                )
            };
        }
        for r in &scalable_values {
            scalable_line = pat::gsub_n(&scalable_line, "#", r, Some(1)).0;
        }
        return Ok(scalable_line);
    }

    // No scalability data: the older way to tell what scales
    let Range::One(range) = *range else {
        return Err("attempt to perform arithmetic on a table value (range)".into());
    };
    let range_pattern = "(%+?)%((%-?%d+%.?%d*)%-(%-?%d+%.?%d*)%)";
    let mut precision_same = true;
    let test_line = if !line.contains('-') {
        line.to_string()
    } else {
        let mut err = None;
        let l = gsub_fn(line, range_pattern, |c| {
            let (min, max) = match (tonum(&c[1]), tonum(&c[2])) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    err = Some(e);
                    return None;
                }
            };
            let max_precision = min + range * (max - min);
            let min_precision = (max_precision + 0.5).floor();
            if min_precision != max_precision {
                precision_same = false;
            }
            let plus = if min_precision < 0.0 { "" } else { &c[0] };
            Some(format!("{plus}{}", num(min_precision)))
        });
        if let Some(e) = err {
            return Err(e);
        }
        gsub_fn(&l, "%-(%d+%%) (%a+)", antonym)
    };
    if precision_same
        && value_scalar.is_none_or(|v| v == 1.0)
        && base_value_scalar.is_none_or(|b| b == 1.0)
    {
        return Ok(test_line);
    }
    let mut precision = None;
    let (list, extra) = parser.parse(test_line.as_bytes());
    if let (Value::Table(t), false) = (&list, extra.truthy()) {
        for m in t.array() {
            let mut sub = m.clone();
            let value = get(&m, "value");
            if value.as_table().is_some() && get(&value, "mod").truthy() {
                sub = get(&value, "mod");
            }
            if let Value::Number(_) = get(&sub, "value") {
                let name = lua::to_string(&get(&sub, "name"))
                    .to_str_lossy()
                    .into_owned();
                let typ = lua::to_string(&get(&sub, "type"))
                    .to_str_lossy()
                    .into_owned();
                if let Some(&p) = data.high_precision_mods.get(&(name, typ)) {
                    precision = Some(p);
                }
            }
        }
    }
    if precision.is_none() && is_match(line, "(%d+%.%d*)") {
        precision = Some(data.default_high_precision);
    }
    let mut numbers = 0;
    let mut err = None;
    let l = gsub_fn(line, range_pattern, |c| {
        numbers += 1;
        let power = 10f64.powf(precision.unwrap_or(0.0));
        let (min, max) = match (tonum(&c[1]), tonum(&c[2])) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                err = Some(e);
                return None;
            }
        };
        let v = ((min + range * (max - min)) * power + 0.5).floor() / power;
        let plus = if v < 0.0 { "" } else { &c[0] };
        Some(format!("{plus}{}", num(v)))
    });
    if let Some(e) = err {
        return Err(e);
    }
    let l = gsub_fn(&l, "%-(%d+%%) (%a+)", antonym);
    if numbers == 0 && is_match(&l, "(%d+%.?%d*)%%? ") {
        numbers = 1;
    }
    apply_value_scalar(&l, value_scalar, base_value_scalar, numbers, precision)
}
