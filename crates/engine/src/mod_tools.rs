//! Port of upstream `src/Modules/ModTools.lua` (v2.67.2).

use lua::{TableRef, Value};

/// `modLib.createMod(modName, modType, modVal, ...)`.
///
/// `rest` is Lua's `...` including trailing nils, since upstream inspects
/// `select('#', ...)`: an optional source string, optional flags and keyword
/// flags, then tags.
pub fn create_mod(name: Value, mod_type: Value, value: Value, rest: &[Value]) -> TableRef {
    let mut flags = Value::Number(0.0);
    let mut keyword_flags = Value::Number(0.0);
    let mut tag_start = 1;
    let mut source = Value::Nil;
    let count = rest.len();
    let sel = |i: usize| rest.get(i - 1).cloned().unwrap_or_default();
    if count >= 1 && matches!(sel(1), Value::Str(_)) {
        source = sel(1);
        tag_start = 2;
    }
    if count >= 2 && matches!(sel(2), Value::Number(_)) {
        flags = sel(2);
        tag_start = 3;
    }
    if count >= 3 && matches!(sel(3), Value::Number(_)) {
        keyword_flags = sel(3);
        tag_start = 4;
    }
    let m = TableRef::new();
    {
        let mut t = m.borrow_mut();
        t.set_str("name", name);
        t.set_str("type", mod_type);
        t.set_str("value", value);
        t.set_str("flags", flags);
        t.set_str("keywordFlags", keyword_flags);
        t.set_str("source", source);
        for (i, v) in rest.iter().skip(tag_start - 1).enumerate() {
            if !v.is_nil() {
                t.set(Value::Number((i + 1) as f64), v.clone());
            }
        }
    }
    m
}

/// Same as [`create_mod`], with the arguments as one slice.
pub fn create_mod_args(args: &[Value]) -> TableRef {
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    create_mod(arg(0), arg(1), arg(2), args.get(3..).unwrap_or(&[]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::*;

    #[test]
    fn create_mod_args_like_upstream() {
        // mod("Life", "BASE", 5)
        let m = create_mod_args(&[s("Life"), s("BASE"), n(5.0)]);
        assert_eq!(m.get_str("flags").as_number(), Some(0.0));
        // mod("Damage", "INC", 10, nil, ModFlag.Attack, KeywordFlag.Hit, tag)
        let tag = tbl(vec![(s("type"), s("Condition"))], vec![]);
        let m = create_mod_args(&[
            s("Damage"),
            s("INC"),
            n(10.0),
            nil(),
            n(1.0),
            n(4.0),
            tag.clone(),
        ]);
        assert_eq!(m.get_str("flags").as_number(), Some(1.0));
        assert_eq!(m.get_str("keywordFlags").as_number(), Some(4.0));
        assert!(m.get_int(1).raw_eq(&tag));
        // mod("X", "BASE", 1, "Source", tag)
        let m = create_mod_args(&[s("X"), s("BASE"), n(1.0), s("Source"), tag.clone()]);
        assert_eq!(m.get_str("source").as_str().unwrap().to_string(), "Source");
        assert!(m.get_int(1).raw_eq(&tag));
        // mod("X", "BASE", 1, nil, nil, tag): tags start at the first extra argument
        let m = create_mod_args(&[s("X"), s("BASE"), n(1.0), nil(), nil(), tag.clone()]);
        assert!(m.get_int(1).is_nil());
        assert!(m.get_int(3).raw_eq(&tag));
    }
}
