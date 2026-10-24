//! Lua support for the port: values, patterns and number formatting with the
//! semantics upstream runs under (LuaJIT 2.1), plus a Lua parser the importer
//! uses to read upstream's data files.

pub mod ast;
pub mod env;
pub mod format;
pub mod gc;
pub mod lexer;
pub mod parser;
pub mod pattern;
pub mod rt;
pub mod value;

pub use value::{
    Closure, FuncRef, Function, Key, LResult, LuaError, LuaStr, Table, TableRef, Value,
};

/// `tonumber(s)` for strings, as LuaJIT's `lj_strscan` reads them: optional
/// surrounding whitespace, decimal or `0x` hex. No `inf` or `nan`.
pub fn str_to_number(s: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(s).ok()?;
    let t = s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
    if t.is_empty() {
        return None;
    }
    let (neg, body) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let v = if body.len() > 2 && (body.starts_with("0x") || body.starts_with("0X")) {
        parse_hex(&body[2..])?
    } else {
        if body.is_empty()
            || !body
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            return None;
        }
        if !body
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == b'.')
        {
            return None;
        }
        body.parse::<f64>().ok()?
    };
    Some(if neg { -v } else { v })
}

fn parse_hex(s: &str) -> Option<f64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut v = 0f64;
    for c in s.bytes() {
        v = v * 16.0 + (c as char).to_digit(16)? as f64;
    }
    Some(v)
}

/// `tonumber(v)`: numbers pass through, strings are converted.
pub fn to_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Str(s) => str_to_number(s.as_bytes()),
        _ => None,
    }
}

/// `string.lower` in the C locale.
pub fn lower(s: &[u8]) -> Vec<u8> {
    s.to_ascii_lowercase()
}

/// `string.upper` in the C locale.
pub fn upper(s: &[u8]) -> Vec<u8> {
    s.to_ascii_uppercase()
}

/// `tostring(v)` for strings and numbers; other types are formatted like Lua
/// would, without addresses.
pub fn to_string(v: &Value) -> LuaStr {
    match v {
        Value::Str(s) => s.clone(),
        Value::Number(n) => value::format_number(*n).into(),
        Value::Nil => "nil".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Table(_) => "table".into(),
        Value::Func(_) | Value::Closure(_) | Value::Function(_) => "function".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonumber() {
        assert_eq!(str_to_number(b"25"), Some(25.0));
        assert_eq!(str_to_number(b" 2.5 "), Some(2.5));
        assert_eq!(str_to_number(b"-3"), Some(-3.0));
        assert_eq!(str_to_number(b".5"), Some(0.5));
        assert_eq!(str_to_number(b"1e3"), Some(1000.0));
        assert_eq!(str_to_number(b"0x10"), Some(16.0));
        assert_eq!(str_to_number(b"inf"), None);
        assert_eq!(str_to_number(b"nan"), None);
        assert_eq!(str_to_number(b"abc"), None);
        assert_eq!(str_to_number(b""), None);
        assert_eq!(str_to_number(b"1-2"), None);
    }
}
