//! JSON with `//` and `/* */` comments, as upstream's `dkjson.decode` reads
//! it, into Lua tables whose keys keep the document's order.

use lua::{TableRef, Value};

pub fn decode(text: &str) -> Result<Value, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        pos: 0,
    };
    let v = p.value()?;
    p.white()?;
    if p.pos != p.s.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        format!("JSON at byte {}: {msg}", self.pos)
    }

    fn white(&mut self) -> Result<(), String> {
        loop {
            let rest = &self.s[self.pos..];
            if rest.first().is_some_and(u8::is_ascii_whitespace) {
                self.pos += 1;
            } else if rest.starts_with(b"//") {
                self.pos += rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
            } else if rest.starts_with(b"/*") {
                let end = rest
                    .windows(2)
                    .position(|w| w == b"*/")
                    .ok_or_else(|| self.err("unterminated comment"))?;
                self.pos += end + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn eat(&mut self, c: u8) -> Result<bool, String> {
        self.white()?;
        let hit = self.s.get(self.pos) == Some(&c);
        if hit {
            self.pos += 1;
        }
        Ok(hit)
    }

    fn value(&mut self) -> Result<Value, String> {
        self.white()?;
        match self.s.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                let t = TableRef::new();
                if !self.eat(b'}')? {
                    loop {
                        self.white()?;
                        let k = self.string()?;
                        if !self.eat(b':')? {
                            return Err(self.err("expected ':'"));
                        }
                        t.set(Value::str(&k), self.value()?);
                        if self.eat(b'}')? {
                            break;
                        }
                        if !self.eat(b',')? {
                            return Err(self.err("expected ',' or '}'"));
                        }
                    }
                }
                Ok(Value::Table(t))
            }
            Some(b'"') => Ok(Value::str(&self.string()?)),
            _ => Err(self.err("only objects and strings are read")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.s.get(self.pos) != Some(&b'"') {
            return Err(self.err("expected a string"));
        }
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let c = *self
                .s
                .get(self.pos)
                .ok_or_else(|| self.err("unterminated string"))?;
            self.pos += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.s.get(self.pos).copied();
                    self.pos += 1;
                    out.push(match e {
                        Some(b'"') => b'"',
                        Some(b'\\') => b'\\',
                        Some(b'/') => b'/',
                        Some(b'n') => b'\n',
                        Some(b't') => b'\t',
                        _ => return Err(self.err("unsupported escape")),
                    });
                }
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("bad UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_order_and_skips_comments() {
        let v =
            super::decode("// c\n{ \"b\": { \"x\": \"1\" }, /* c */ \"a\": \"\\\"2\" }").unwrap();
        let t = v.as_table().unwrap();
        let keys: Vec<_> = t.entries().iter().map(|(k, _)| format!("{k:?}")).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].contains('b') && keys[1].contains('a'));
        assert_eq!(t.get_str("a").as_str().unwrap().to_str_lossy(), "\"2");
    }
}
