//! Lua 5.1 patterns (`string.find`, `match`, `gmatch`, `gsub`), ported from
//! `lstrlib.c`, which LuaJIT's string library follows. Works on bytes with
//! C-locale character classes, as upstream runs.
//!
//! Positions in this module are 0-based byte offsets; the `lua_*` wrappers
//! take and return Lua's 1-based positions.

use crate::value::{LuaStr, Value};

const MAX_CAPTURES: usize = 32;
const L_ESC: u8 = b'%';
const SPECIALS: &[u8] = b"^$*+?.([%-";

#[derive(Debug, Clone, PartialEq)]
pub enum Capture {
    /// Byte range of a string capture
    Str(usize, usize),
    /// 0-based position of a `()` capture
    Pos(usize),
}

impl Capture {
    /// The capture as a Lua value; positions become 1-based numbers.
    pub fn to_value(&self, s: &[u8]) -> Value {
        match *self {
            Capture::Str(a, b) => Value::Str(LuaStr::new(&s[a..b])),
            Capture::Pos(p) => Value::Number((p + 1) as f64),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum PatternError {
    MalformedEnd,
    MissingBracket,
    MissingBalanceArgs,
    InvalidCaptureIndex,
    InvalidPatternCapture,
    UnfinishedCapture,
    TooManyCaptures,
    TooComplex,
    InvalidReplacement,
}

pub type Result<T> = std::result::Result<T, PatternError>;

#[derive(Clone, Copy)]
enum CapLen {
    Len(usize),
    Unfinished,
    Position,
}

struct MatchState<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    level: usize,
    capture: [(usize, CapLen); MAX_CAPTURES],
    depth: usize,
}

const MAX_DEPTH: usize = 200_000;

fn class_end(ms: &MatchState, mut p: usize) -> Result<usize> {
    let pat = ms.pat;
    let c = pat[p];
    p += 1;
    if c == L_ESC {
        if p >= pat.len() {
            return Err(PatternError::MalformedEnd);
        }
        return Ok(p + 1);
    }
    if c == b'[' {
        if p < pat.len() && pat[p] == b'^' {
            p += 1;
        }
        // Look for a ']'; the first character after '[' or '[^' is literal
        loop {
            if p >= pat.len() {
                return Err(PatternError::MissingBracket);
            }
            let cc = pat[p];
            p += 1;
            if cc == L_ESC {
                if p >= pat.len() {
                    return Err(PatternError::MissingBracket);
                }
                p += 1;
            }
            if p < pat.len() && pat[p] == b']' {
                return Ok(p + 1);
            }
            if p >= pat.len() {
                return Err(PatternError::MissingBracket);
            }
        }
    }
    Ok(p)
}

/// `match_class` from lstrlib.c with C-locale ctype.
fn match_class(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,
        _ => return cl == c,
    };
    if cl.is_ascii_uppercase() { !res } else { res }
}

/// `matchbracketclass`: `p` is at '[', `ec` at the closing ']'.
fn match_bracket_class(ms: &MatchState, c: u8, mut p: usize, ec: usize) -> bool {
    let pat = ms.pat;
    let mut sig = true;
    if pat[p + 1] == b'^' {
        sig = false;
        p += 1;
    }
    p += 1;
    while p < ec {
        if pat[p] == L_ESC {
            p += 1;
            if match_class(c, pat[p]) {
                return sig;
            }
            p += 1;
        } else if p + 2 < ec && pat[p + 1] == b'-' {
            if pat[p] <= c && c <= pat[p + 2] {
                return sig;
            }
            p += 3;
        } else {
            if pat[p] == c {
                return sig;
            }
            p += 1;
        }
    }
    !sig
}

fn single_match(ms: &MatchState, s: usize, p: usize, ep: usize) -> bool {
    if s >= ms.src.len() {
        return false;
    }
    let c = ms.src[s];
    match ms.pat[p] {
        b'.' => true,
        L_ESC => match_class(c, ms.pat[p + 1]),
        b'[' => match_bracket_class(ms, c, p, ep - 1),
        pc => pc == c,
    }
}

fn match_balance(ms: &MatchState, s: usize, p: usize) -> Result<Option<usize>> {
    if p + 1 >= ms.pat.len() {
        return Err(PatternError::MissingBalanceArgs);
    }
    if s >= ms.src.len() || ms.src[s] != ms.pat[p] {
        return Ok(None);
    }
    let b = ms.pat[p];
    let e = ms.pat[p + 1];
    let mut cont = 1;
    let mut i = s + 1;
    while i < ms.src.len() {
        let c = ms.src[i];
        if c == e {
            cont -= 1;
            if cont == 0 {
                return Ok(Some(i + 1));
            }
        } else if c == b {
            cont += 1;
        }
        i += 1;
    }
    Ok(None)
}

fn max_expand(ms: &mut MatchState, s: usize, p: usize, ep: usize) -> Result<Option<usize>> {
    let mut i = 0usize;
    while single_match(ms, s + i, p, ep) {
        i += 1;
    }
    // Try with the maximum repetitions, then fewer
    loop {
        if let Some(res) = do_match(ms, s + i, ep + 1)? {
            return Ok(Some(res));
        }
        if i == 0 {
            return Ok(None);
        }
        i -= 1;
    }
}

fn min_expand(ms: &mut MatchState, mut s: usize, p: usize, ep: usize) -> Result<Option<usize>> {
    loop {
        if let Some(res) = do_match(ms, s, ep + 1)? {
            return Ok(Some(res));
        }
        if single_match(ms, s, p, ep) {
            s += 1;
        } else {
            return Ok(None);
        }
    }
}

fn start_capture(ms: &mut MatchState, s: usize, p: usize, what: CapLen) -> Result<Option<usize>> {
    if ms.level >= MAX_CAPTURES {
        return Err(PatternError::TooManyCaptures);
    }
    ms.capture[ms.level] = (s, what);
    ms.level += 1;
    let res = do_match(ms, s, p)?;
    if res.is_none() {
        ms.level -= 1;
    }
    Ok(res)
}

fn end_capture(ms: &mut MatchState, s: usize, p: usize) -> Result<Option<usize>> {
    let l = capture_to_close(ms)?;
    ms.capture[l].1 = CapLen::Len(s - ms.capture[l].0);
    let res = do_match(ms, s, p)?;
    if res.is_none() {
        ms.capture[l].1 = CapLen::Unfinished;
    }
    Ok(res)
}

fn capture_to_close(ms: &MatchState) -> Result<usize> {
    let mut level = ms.level as isize - 1;
    while level >= 0 {
        if matches!(ms.capture[level as usize].1, CapLen::Unfinished) {
            return Ok(level as usize);
        }
        level -= 1;
    }
    Err(PatternError::InvalidPatternCapture)
}

fn check_capture(ms: &MatchState, l: u8) -> Result<usize> {
    let l = l.wrapping_sub(b'1') as usize;
    if l >= ms.level || matches!(ms.capture[l].1, CapLen::Unfinished) {
        return Err(PatternError::InvalidCaptureIndex);
    }
    Ok(l)
}

fn match_capture(ms: &MatchState, s: usize, l: u8) -> Result<Option<usize>> {
    let l = check_capture(ms, l)?;
    let (start, len) = ms.capture[l];
    let len = match len {
        CapLen::Len(n) => n,
        _ => return Err(PatternError::InvalidCaptureIndex),
    };
    if ms.src.len() - s >= len && ms.src[start..start + len] == ms.src[s..s + len] {
        Ok(Some(s + len))
    } else {
        Ok(None)
    }
}

/// `do_match` from lstrlib.c. Returns the end of the match.
fn do_match(ms: &mut MatchState, mut s: usize, mut p: usize) -> Result<Option<usize>> {
    ms.depth += 1;
    if ms.depth > MAX_DEPTH {
        return Err(PatternError::TooComplex);
    }
    let res = loop {
        if p >= ms.pat.len() {
            break Ok(Some(s));
        }
        match ms.pat[p] {
            b'(' => {
                break if p + 1 < ms.pat.len() && ms.pat[p + 1] == b')' {
                    start_capture(ms, s, p + 2, CapLen::Position)
                } else {
                    start_capture(ms, s, p + 1, CapLen::Unfinished)
                };
            }
            b')' => break end_capture(ms, s, p + 1),
            L_ESC if p + 1 < ms.pat.len() && ms.pat[p + 1] == b'b' => {
                match match_balance(ms, s, p + 2)? {
                    Some(ns) => {
                        s = ns;
                        p += 4;
                        continue;
                    }
                    None => break Ok(None),
                }
            }
            L_ESC if p + 1 < ms.pat.len() && ms.pat[p + 1] == b'f' => {
                p += 2;
                if p >= ms.pat.len() || ms.pat[p] != b'[' {
                    return Err(PatternError::MissingBracket);
                }
                let ep = class_end(ms, p)?;
                let prev = if s == 0 { 0 } else { ms.src[s - 1] };
                let cur = if s < ms.src.len() { ms.src[s] } else { 0 };
                if !match_bracket_class(ms, prev, p, ep - 1)
                    && match_bracket_class(ms, cur, p, ep - 1)
                {
                    p = ep;
                    continue;
                }
                break Ok(None);
            }
            L_ESC if p + 1 < ms.pat.len() && ms.pat[p + 1].is_ascii_digit() => {
                match match_capture(ms, s, ms.pat[p + 1])? {
                    Some(ns) => {
                        s = ns;
                        p += 2;
                        continue;
                    }
                    None => break Ok(None),
                }
            }
            b'$' if p + 1 == ms.pat.len() => {
                break Ok(if s == ms.src.len() { Some(s) } else { None });
            }
            _ => {
                let ep = class_end(ms, p)?;
                let m = single_match(ms, s, p, ep);
                let next = ms.pat.get(ep).copied();
                match next {
                    Some(b'?') => {
                        if m && let Some(res) = do_match(ms, s + 1, ep + 1)? {
                            break Ok(Some(res));
                        }
                        p = ep + 1;
                        continue;
                    }
                    Some(b'*') => break max_expand(ms, s, p, ep),
                    Some(b'+') => {
                        break if m {
                            max_expand(ms, s + 1, p, ep)
                        } else {
                            Ok(None)
                        };
                    }
                    Some(b'-') => break min_expand(ms, s, p, ep),
                    _ => {
                        if !m {
                            break Ok(None);
                        }
                        s += 1;
                        p = ep;
                        continue;
                    }
                }
            }
        }
    };
    ms.depth -= 1;
    res
}

fn get_captures(ms: &MatchState, s: usize, e: usize, whole_if_none: bool) -> Result<Vec<Capture>> {
    let n = if ms.level == 0 && whole_if_none {
        1
    } else {
        ms.level
    };
    let mut caps = Vec::with_capacity(n);
    for i in 0..n {
        caps.push(get_onecapture(ms, i, s, e)?);
    }
    Ok(caps)
}

fn get_onecapture(ms: &MatchState, i: usize, s: usize, e: usize) -> Result<Capture> {
    if i >= ms.level {
        if i == 0 {
            return Ok(Capture::Str(s, e));
        }
        return Err(PatternError::InvalidCaptureIndex);
    }
    let (start, len) = ms.capture[i];
    match len {
        CapLen::Position => Ok(Capture::Pos(start)),
        CapLen::Len(l) => Ok(Capture::Str(start, start + l)),
        CapLen::Unfinished => Err(PatternError::UnfinishedCapture),
    }
}

fn new_state<'a>(src: &'a [u8], pat: &'a [u8]) -> MatchState<'a> {
    MatchState {
        src,
        pat,
        level: 0,
        capture: [(0, CapLen::Unfinished); MAX_CAPTURES],
        depth: 0,
    }
}

/// A match: byte range of the whole match plus captures.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub captures: Vec<Capture>,
}

/// Finds the first match at or after `init` (0-based). With `find_mode`, a
/// pattern without special characters is searched as plain text, as
/// `string.find` does; captures are returned without the whole match.
fn str_find_aux(
    s: &[u8],
    p: &[u8],
    init: usize,
    plain: bool,
    find_mode: bool,
) -> Result<Option<Match>> {
    if init > s.len() {
        return Ok(None);
    }
    if find_mode && (plain || !p.iter().any(|c| SPECIALS.contains(c))) {
        if p.is_empty() {
            return Ok(Some(Match {
                start: init,
                end: init,
                captures: vec![],
            }));
        }
        return Ok(s[init..]
            .windows(p.len())
            .position(|w| w == p)
            .map(|i| Match {
                start: init + i,
                end: init + i + p.len(),
                captures: vec![],
            }));
    }
    let (anchor, pat) = if p.first() == Some(&b'^') {
        (true, &p[1..])
    } else {
        (false, p)
    };
    let mut s1 = init;
    loop {
        let mut ms = new_state(s, pat);
        if let Some(e) = do_match(&mut ms, s1, 0)? {
            let captures = get_captures(&ms, s1, e, !find_mode)?;
            return Ok(Some(Match {
                start: s1,
                end: e,
                captures,
            }));
        }
        s1 += 1;
        if anchor || s1 > s.len() {
            return Ok(None);
        }
    }
}

/// Lua's position argument handling (`posrelat` plus clamping) for find/match.
fn init_pos(init: i64, len: usize) -> usize {
    let len = len as i64;
    let mut i = if init < 0 { len + init + 1 } else { init };
    if i < 1 {
        i = 1;
    }
    (i - 1) as usize
}

/// `string.find(s, p, init, plain)`: 1-based start and end, plus captures.
pub fn lua_find(
    s: &[u8],
    p: &[u8],
    init: i64,
    plain: bool,
) -> Result<Option<(usize, usize, Vec<Value>)>> {
    let start = init_pos(init, s.len());
    Ok(str_find_aux(s, p, start, plain, true)?.map(|m| {
        (
            m.start + 1,
            m.end,
            m.captures.iter().map(|c| c.to_value(s)).collect(),
        )
    }))
}

/// `string.match(s, p, init)`: the captures, or the whole match.
pub fn lua_match(s: &[u8], p: &[u8], init: i64) -> Result<Option<Vec<Value>>> {
    let start = init_pos(init, s.len());
    Ok(str_find_aux(s, p, start, false, false)?
        .map(|m| m.captures.iter().map(|c| c.to_value(s)).collect()))
}

/// `string.gmatch(s, p)`: every match's captures, or whole matches.
pub fn lua_gmatch(s: &[u8], p: &[u8]) -> Result<Vec<Vec<Value>>> {
    let mut out = Vec::new();
    let mut src = 0;
    while src <= s.len() {
        let mut ms = new_state(s, p);
        match do_match(&mut ms, src, 0)? {
            Some(e) => {
                let caps = get_captures(&ms, src, e, true)?;
                out.push(caps.iter().map(|c| c.to_value(s)).collect());
                // Empty match: advance by one
                src = if e == src { src + 1 } else { e };
            }
            None => src += 1,
        }
    }
    Ok(out)
}

/// Replacement for `gsub`.
pub enum Repl<'a> {
    /// Replacement string with `%0`-`%9` and `%%`
    Str(&'a [u8]),
    /// Called with the captures (or the whole match); nil or false keeps the
    /// original text, a string or number replaces it
    Func(&'a mut dyn FnMut(&[Value]) -> Value),
}

/// `string.gsub(s, p, repl, n)`: the new string and the number of matches.
pub fn lua_gsub(
    s: &[u8],
    p: &[u8],
    mut repl: Repl,
    max_n: Option<usize>,
) -> Result<(Vec<u8>, usize)> {
    let (anchor, pat) = if p.first() == Some(&b'^') {
        (true, &p[1..])
    } else {
        (false, p)
    };
    let max_n = max_n.unwrap_or(s.len() + 1);
    let mut out = Vec::with_capacity(s.len());
    let mut src = 0;
    let mut n = 0;
    while n < max_n {
        let mut ms = new_state(s, pat);
        let e = do_match(&mut ms, src, 0)?;
        if let Some(e) = e {
            n += 1;
            add_value(&ms, &mut out, src, e, &mut repl)?;
        }
        match e {
            Some(e) if e > src => src = e,
            _ => {
                if src < s.len() {
                    out.push(s[src]);
                    src += 1;
                } else {
                    break;
                }
            }
        }
        if anchor {
            break;
        }
    }
    out.extend_from_slice(&s[src.min(s.len())..]);
    Ok((out, n))
}

fn add_value(
    ms: &MatchState,
    out: &mut Vec<u8>,
    s: usize,
    e: usize,
    repl: &mut Repl,
) -> Result<()> {
    match repl {
        Repl::Str(news) => {
            let mut i = 0;
            while i < news.len() {
                if news[i] == L_ESC {
                    i += 1;
                    let c = *news.get(i).ok_or(PatternError::InvalidReplacement)?;
                    if !c.is_ascii_digit() {
                        // LuaJIT, like Lua 5.1, copies the character after a
                        // stray '%' literally
                        out.push(c);
                    } else if c == b'0' {
                        out.extend_from_slice(&ms.src[s..e]);
                    } else {
                        let cap = get_onecapture(ms, (c - b'1') as usize, s, e)?;
                        push_value(out, &cap.to_value(ms.src));
                    }
                } else {
                    out.push(news[i]);
                }
                i += 1;
            }
            Ok(())
        }
        Repl::Func(f) => {
            let caps = get_captures(ms, s, e, true)?;
            let args: Vec<Value> = caps.iter().map(|c| c.to_value(ms.src)).collect();
            let v = f(&args);
            match v {
                Value::Nil | Value::Bool(false) => out.extend_from_slice(&ms.src[s..e]),
                Value::Str(_) | Value::Number(_) => push_value(out, &v),
                _ => return Err(PatternError::InvalidReplacement),
            }
            Ok(())
        }
    }
}

fn push_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Str(s) => out.extend_from_slice(s.as_bytes()),
        Value::Number(n) => out.extend_from_slice(crate::value::format_number(*n).as_bytes()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(s: &str, p: &str) -> Option<(usize, usize, Vec<String>)> {
        lua_find(s.as_bytes(), p.as_bytes(), 1, false)
            .unwrap()
            .map(|(a, b, caps)| (a, b, caps.iter().map(|c| format!("{c:?}")).collect()))
    }

    #[test]
    fn find_basics() {
        assert_eq!(find("hello world", "wor"), Some((7, 9, vec![])));
        assert_eq!(find("hello world", "o w"), Some((5, 7, vec![])));
        assert_eq!(find("hello", "^h(.)"), Some((1, 2, vec!["\"e\"".into()])));
        assert_eq!(find("hello", "^e"), None);
        assert_eq!(
            find("+25% increased", "^([%+%-]?[%d%.]+)%%? "),
            Some((1, 5, vec!["\"+25\"".into()]))
        );
        assert_eq!(
            find("25% increased", "(%d+)%% (%a+)"),
            Some((1, 13, vec!["\"25\"".into(), "\"increased\"".into()]))
        );
        assert_eq!(
            find("abc", "()b()"),
            Some((2, 2, vec!["2".into(), "3".into()]))
        );
        assert_eq!(find("x = [a]", "%b[]"), Some((5, 7, vec![])));
        assert_eq!(find("THE (quick) fox", "%f[%a]%a+"), Some((1, 3, vec![])));
        assert_eq!(find("aaa", "a-$"), Some((1, 3, vec![])));
        assert_eq!(find("ab", "[^%s]+"), Some((1, 2, vec![])));
        assert_eq!(find("a]b", "[]]"), Some((2, 2, vec![])));
        assert_eq!(find("a-b", "[a-]+"), Some((1, 2, vec![])));
    }

    #[test]
    fn match_and_gmatch() {
        let m = lua_match(b"key = value", b"(%w+) = (%w+)", 1)
            .unwrap()
            .unwrap();
        assert_eq!(format!("{m:?}"), "[\"key\", \"value\"]");
        let all = lua_gmatch(b"one two  three", b"%a+").unwrap();
        assert_eq!(all.len(), 3);
        let lines = lua_gmatch(b"a\nb\n\nc", b"[^\n]+").unwrap();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn gsub_basics() {
        let (s, n) = lua_gsub(b"hello world", b"o", Repl::Str(b"0"), None).unwrap();
        assert_eq!((s.as_slice(), n), (&b"hell0 w0rld"[..], 2));
        let (s, _) = lua_gsub(b"abc", b"", Repl::Str(b"-"), None).unwrap();
        assert_eq!(s, b"-a-b-c-");
        let (s, _) = lua_gsub(b"x y", b"(%w)", Repl::Str(b"<%1%%>"), None).unwrap();
        assert_eq!(s, b"<x%> <y%>");
        let mut f = |caps: &[Value]| match caps[0].as_str() {
            Some(s) if s.as_bytes() == b"a" => Value::str("A"),
            _ => Value::Nil,
        };
        let (s, n) = lua_gsub(b"a b a", b"%a", Repl::Func(&mut f), None).unwrap();
        assert_eq!((s.as_slice(), n), (&b"A b A"[..], 3));
        let (s, n) = lua_gsub(b"aaa", b"^a", Repl::Str(b"b"), None).unwrap();
        assert_eq!((s.as_slice(), n), (&b"baa"[..], 1));
        let (s, _) = lua_gsub(b"1 2 3", b"%d", Repl::Str(b"x"), Some(2)).unwrap();
        assert_eq!(s, b"x x 3");
        let (s, _) = lua_gsub(b"x", b"x", Repl::Str(b"%a"), None).unwrap();
        assert_eq!(s, b"a");
    }
}
