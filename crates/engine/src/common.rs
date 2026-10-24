//! Port of helpers from upstream `src/Modules/Common.lua` (v2.67.2).

use crate::rt::*;
use lua::Value;

/// `sanitiseText(text)`
pub fn sanitise_text(text: &Value) -> Value {
    let Some(s) = text.as_str() else {
        return text.clone();
    };
    if !s.as_bytes().iter().any(|&b| b >= 128 || b == b'<') {
        return text.clone();
    }
    let mut v = first(gsub(text, &s_("%b<>"), &s_(""), &nil()));
    for (from, to) in [
        (&b"\xe2\x80\x90"[..], "-"),
        (b"\xe2\x80\x91", "-"),
        (b"\xe2\x80\x92", "-"),
        (b"\xe2\x80\x93", "-"),
        (b"\xe2\x80\x94", "-"),
        (b"\xe2\x80\x95", "-"),
        (b"\xe2\x88\x92", "-"),
        (b"\xc3\xa4", "a"),
        (b"\xc3\xb6", "o"),
        (b"\x96", "-"),
        (b"\x97", "-"),
        (b"\xe4", "a"),
        (b"\xf6", "o"),
    ] {
        v = first(gsub(&v, &sb(from), &s_(to), &nil()));
    }
    first(gsub(&v, &sb(b"[\x80-\xff]"), &s_("?"), &nil()))
}

fn s_(x: &str) -> Value {
    s(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise() {
        assert_eq!(
            sanitise_text(&s("plain")).as_str().unwrap().to_string(),
            "plain"
        );
        assert_eq!(
            sanitise_text(&s("a<b>c")).as_str().unwrap().to_string(),
            "ac"
        );
        assert_eq!(
            sanitise_text(&s("x\u{2013}y"))
                .as_str()
                .unwrap()
                .to_string(),
            "x-y"
        );
        assert_eq!(
            sanitise_text(&s("Maj\u{f6}r"))
                .as_str()
                .unwrap()
                .to_string(),
            "Major"
        );
        assert_eq!(
            sanitise_text(&s("\u{e9}")).as_str().unwrap().to_string(),
            "??"
        );
    }
}
