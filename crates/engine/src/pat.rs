//! Text matching with the patterns upstream writes (Lua 5.1 patterns), on
//! Rust strings: what `string.find`, `match`, `gmatch` and `gsub` do in
//! upstream. Item text and data lines are matched with upstream's own
//! patterns, so the results are upstream's.

use lua::Value;
use lua::pattern::{self, Repl};

fn text(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_str_lossy().into_owned(),
        Value::Number(n) => lua::value::format_number(*n),
        _ => String::new(),
    }
}

fn bad(p: &str) -> impl Fn(pattern::PatternError) -> String + '_ {
    move |e| format!("bad pattern {p:?}: {e:?}")
}

/// `s:find(p)`: the 0-based byte range of the first match.
pub fn find(s: &str, p: &str) -> Option<(usize, usize)> {
    pattern::lua_find(s.as_bytes(), p.as_bytes(), 1, false)
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)))
        .map(|(a, b, _)| (a - 1, b))
}

/// `s:find(p)` with a pattern from text, which may be malformed: the error
/// upstream raises then
pub fn try_find(s: &str, p: &str) -> Result<Option<(usize, usize)>, String> {
    pattern::lua_find(s.as_bytes(), p.as_bytes(), 1, false)
        .map(|m| m.map(|(a, b, _)| (a - 1, b)))
        .map_err(bad(p))
}

/// Whether `s:find(p)` matches
pub fn is_match(s: &str, p: &str) -> bool {
    find(s, p).is_some()
}

/// `s:match(p)`: the captures, or the whole match.
pub fn captures(s: &str, p: &str) -> Option<Vec<String>> {
    pattern::lua_match(s.as_bytes(), p.as_bytes(), 1)
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)))
        .map(|caps| caps.iter().map(text).collect())
}

/// `s:match(p, init)` from the 1-based position `init`; position
/// captures (`()`) give their position as text
pub fn captures_at(s: &str, p: &str, init: usize) -> Option<Vec<String>> {
    pattern::lua_match(s.as_bytes(), p.as_bytes(), init as i64)
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)))
        .map(|caps| caps.iter().map(text).collect())
}

/// The first capture of `s:match(p)` (or the whole match)
pub fn capture(s: &str, p: &str) -> Option<String> {
    captures(s, p).and_then(|c| c.into_iter().next())
}

/// `s:gmatch(p)`: every match's captures, or whole matches.
pub fn gmatch(s: &str, p: &str) -> Vec<Vec<String>> {
    pattern::lua_gmatch(s.as_bytes(), p.as_bytes())
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)))
        .iter()
        .map(|caps| caps.iter().map(text).collect())
        .collect()
}

/// The first capture of each match of `s:gmatch(p)`
pub fn gmatch1(s: &str, p: &str) -> Vec<String> {
    gmatch(s, p)
        .into_iter()
        .filter_map(|c| c.into_iter().next())
        .collect()
}

/// `s:gsub(p, repl, n)` with a replacement string (`%1`, `%%`): the new
/// string and the number of matches.
pub fn gsub_n(s: &str, p: &str, repl: &str, n: Option<usize>) -> (String, usize) {
    let (out, count) = pattern::lua_gsub(s.as_bytes(), p.as_bytes(), Repl::Str(repl.as_bytes()), n)
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)));
    (String::from_utf8_lossy(&out).into_owned(), count)
}

/// `s:gsub(p, repl)`
pub fn gsub(s: &str, p: &str, repl: &str) -> String {
    gsub_n(s, p, repl, None).0
}

/// `s:gsub(p, f, n)`: `f` gets the captures (or the whole match); `None`
/// keeps the matched text.
pub fn gsub_fn_n(
    s: &str,
    p: &str,
    n: Option<usize>,
    mut f: impl FnMut(&[String]) -> Option<String>,
) -> (String, usize) {
    let mut call = |caps: &[Value]| -> Value {
        let caps: Vec<String> = caps.iter().map(text).collect();
        match f(&caps) {
            Some(r) => Value::str(&r),
            None => Value::Nil,
        }
    };
    let (out, count) = pattern::lua_gsub(s.as_bytes(), p.as_bytes(), Repl::Func(&mut call), n)
        .unwrap_or_else(|e| panic!("{}", bad(p)(e)));
    (String::from_utf8_lossy(&out).into_owned(), count)
}

/// `s:gsub(p, f)`
pub fn gsub_fn(s: &str, p: &str, f: impl FnMut(&[String]) -> Option<String>) -> String {
    gsub_fn_n(s, p, None, f).0
}

/// `s:find(sub, 1, true)`: a plain find, 0-based
pub fn find_plain(s: &str, sub: &str) -> Option<(usize, usize)> {
    s.find(sub).map(|i| (i, i + sub.len()))
}

/// Escapes the magic characters of `s` (`s:gsub("%p", "%%%0")` without
/// touching letters and digits), for building a pattern from text
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "^$()%.[]*+-?".contains(c) {
            out.push('%');
        }
        out.push(c);
    }
    out
}

/// Lua's `tostring` of a number (`%.14g`)
pub fn num(n: f64) -> String {
    lua::value::format_number(n)
}

/// Lua's `tonumber` of a string
pub fn to_number(s: &str) -> Option<f64> {
    lua::str_to_number(s.as_bytes())
}

/// `s:lower()` (ASCII, as the C locale)
pub fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// `string.format("%.{precision}f", n)`
pub fn format_f(n: f64, precision: usize) -> String {
    lua::format::format_f(n, precision)
}

/// `formatNumSep(str)` with upstream's default separators: thousands
/// grouped with `,` in each number, colour codes kept
pub fn format_num_sep(s: &str) -> String {
    gsub_fn(s, "(%^?x?%x?%x?%x?%x?%x?%x?-?%d+%.?%d+)", |c| {
        let m = &c[0];
        let colour = capture(m, "(^x%x%x%x%x%x%x)")
            .or_else(|| capture(m, "(%^%d)"))
            .unwrap_or_default();
        let bare = gsub(&gsub(m, "(^x%x%x%x%x%x%x)", ""), "(%^%d)", "");
        if bare.is_empty() || (colour.is_empty() && m.contains('^')) {
            return None;
        }
        let parts = captures(&bare, "(-?)(%d+)(%.?%d*)")?;
        let int = parts[1].as_bytes();
        let mut grouped = String::new();
        for (i, d) in int.iter().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(*d as char);
        }
        Some(format!("{colour}{}{grouped}{}", parts[0], parts[2]))
    })
}

/// `string:matchOrPattern(pattern)` (`Modules/Common.lua`): whether `s`
/// matches one of the patterns that `(a|b)` groups and `|` make of
/// `pattern`. Upstream tries them in `pairs` order and stops at the first
/// match or at a malformed one (its search's `PCall` then fails); here
/// they are tried in the order they are made.
pub fn match_or_pattern(s: &str, pattern: &str) -> Result<bool, String> {
    fn generate(p: &str) -> Vec<String> {
        let mut groups: Vec<(usize, usize, Vec<String>)> = Vec::new();
        for g in gmatch1(p, "%b()") {
            let init = groups.last().map_or(1, |g| g.1);
            let Some(at) = p[init - 1..].find(g.as_str()) else {
                continue;
            };
            let open = init + at;
            let close = open + g.len() - 1;
            groups.push((open, close, generate(&g[1..g.len() - 1])));
        }
        // `pattern:sub(i, j)` with 1-based, inclusive, -1 for the end
        let sub = |i: usize, j: Option<usize>| -> String {
            let end = j.unwrap_or(p.len()).min(p.len());
            if i > end {
                String::new()
            } else {
                p[i - 1..end].to_string()
            }
        };
        let mut generated = vec![sub(1, groups.first().map(|g| g.0 - 1))];
        for (i, (_, close, patterns)) in groups.iter().enumerate() {
            let next = sub(close + 1, groups.get(i + 1).map(|g| g.0 - 1));
            let mut temp = Vec::new();
            for sp in &generated {
                for gp in patterns {
                    temp.push(format!("{sp}{gp}{next}"));
                }
            }
            generated = temp;
        }
        let mut out: Vec<String> = Vec::new();
        for g in &generated {
            for o in g.split('|').filter(|o| !o.is_empty()) {
                if !out.iter().any(|x| x == o) {
                    out.push(o.to_string());
                }
            }
        }
        out
    }
    for p in generate(pattern) {
        if try_find(s, &p)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
