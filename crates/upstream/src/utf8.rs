//! The parts of the `lua-utf8` library upstream uses. Indices are in
//! characters; patterns match bytes, which is the same for the ASCII text
//! (numbers and separators) upstream uses them on.

use lua::rt::{call, err, first, func};
use lua::{LResult, TableRef, Value};

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or_default()
}

fn text(a: &[Value], i: usize) -> LResult<String> {
    match a.get(i) {
        Some(v @ (Value::Str(_) | Value::Number(_))) => Ok(lua::to_string(v).to_string()),
        _ => err(format!(
            "bad argument #{} to utf8 function (string expected)",
            i + 1
        )),
    }
}

/// Calls the string library function of the same name.
fn via_string(name: &'static str) -> Value {
    func(name, move |a| {
        let string = match lua::rt::index(&Value::str("x"), &Value::str(name))? {
            Value::Nil => return err(format!("string.{name} missing")),
            f => f,
        };
        call(&string, a)
    })
}

pub fn library() -> Value {
    let t = TableRef::new();
    t.set_str("gsub", via_string("gsub"));
    t.set_str("match", via_string("match"));
    t.set_str("find", via_string("find"));
    t.set_str(
        "reverse",
        func("utf8.reverse", |a| {
            Ok(vec![Value::str(
                &text(a, 0)?.chars().rev().collect::<String>(),
            )])
        }),
    );
    t.set_str(
        "len",
        func("utf8.len", |a| {
            Ok(vec![Value::Number(text(a, 0)?.chars().count() as f64)])
        }),
    );
    t.set_str(
        "sub",
        func("utf8.sub", |a| {
            let s: Vec<char> = text(a, 0)?.chars().collect();
            let len = s.len() as i64;
            let num = |i: usize, d: i64| lua::to_number(&arg(a, i)).map(|n| n as i64).unwrap_or(d);
            let (mut i, mut j) = (num(1, 1), num(2, -1));
            if i < 0 {
                i = (len + i + 1).max(1);
            }
            if j < 0 {
                j = len + j + 1;
            }
            i = i.max(1);
            j = j.min(len);
            if i > j {
                return Ok(vec![Value::str("")]);
            }
            Ok(vec![Value::str(
                &s[i as usize - 1..j as usize].iter().collect::<String>(),
            )])
        }),
    );
    t.set_str(
        "next",
        func("utf8.next", |a| {
            // Byte position of the next (or previous) character start
            let s = text(a, 0)?;
            let b = s.as_bytes();
            let pos = lua::to_number(&arg(a, 1)).unwrap_or(0.0) as i64;
            let dir = lua::to_number(&arg(a, 2)).unwrap_or(1.0) as i64;
            let is_start =
                |p: i64| p >= 1 && p as usize <= b.len() && (b[p as usize - 1] & 0xc0) != 0x80;
            let mut p = pos + dir;
            while p >= 1 && p as usize <= b.len() && !is_start(p) {
                p += dir;
            }
            if is_start(p) {
                Ok(vec![Value::Number(p as f64)])
            } else {
                Ok(vec![Value::Nil])
            }
        }),
    );
    let _ = first;
    Value::Table(t)
}
