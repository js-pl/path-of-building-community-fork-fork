//! Lua 5.1 lexer (plus LuaJIT's `goto` and `::label::`).

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Name(String),
    Str(Vec<u8>),
    Num(f64),
    // Keywords
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    Goto,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    // Symbols
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Hash,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    DoubleColon,
    Semi,
    Colon,
    Comma,
    Dot,
    Concat,
    Dots,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Byte offset of the token's start
    pub start: usize,
    /// Byte offset just past the token
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub line: u32,
    pub msg: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

pub fn tokenize(src: &[u8]) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer {
        src,
        pos: 0,
        line: 1,
    };
    let mut out = Vec::new();
    // Skip a shebang line
    if src.starts_with(b"#") {
        while lx.pos < src.len() && src[lx.pos] != b'\n' {
            lx.pos += 1;
        }
    }
    loop {
        let t = lx.next()?;
        let eof = t.tok == Tok::Eof;
        out.push(t);
        if eof {
            return Ok(out);
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
}

impl Lexer<'_> {
    fn peek(&self, off: usize) -> u8 {
        *self.src.get(self.pos + off).unwrap_or(&0)
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, LexError> {
        Err(LexError {
            line: self.line,
            msg: msg.into(),
        })
    }

    fn skip_space_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            let c = self.peek(0);
            match c {
                b'\n' => {
                    self.line += 1;
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' | 0x0b | 0x0c => self.pos += 1,
                b'-' if self.peek(1) == b'-' => {
                    self.pos += 2;
                    if self.peek(0) == b'['
                        && let Some(level) = self.long_bracket_level()
                    {
                        self.read_long_string(level)?;
                        continue;
                    }
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// At '[': the level of a long bracket `[==[`, if this is one.
    fn long_bracket_level(&self) -> Option<usize> {
        let mut i = 1;
        while self.peek(i) == b'=' {
            i += 1;
        }
        if self.peek(i) == b'[' {
            Some(i - 1)
        } else {
            None
        }
    }

    fn read_long_string(&mut self, level: usize) -> Result<Vec<u8>, LexError> {
        self.pos += level + 2;
        // A newline right after the opening bracket is skipped
        if self.peek(0) == b'\r' {
            self.pos += 1;
            if self.peek(0) == b'\n' {
                self.pos += 1;
            }
            self.line += 1;
        } else if self.peek(0) == b'\n' {
            self.pos += 1;
            if self.peek(0) == b'\r' {
                self.pos += 1;
            }
            self.line += 1;
        }
        let mut out = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                return self.err("unfinished long string");
            }
            let c = self.src[self.pos];
            if c == b']' {
                let mut i = 1;
                while self.peek(i) == b'=' {
                    i += 1;
                }
                if i - 1 == level && self.peek(i) == b']' {
                    self.pos += i + 1;
                    return Ok(out);
                }
            }
            if c == b'\n' {
                self.line += 1;
            }
            out.push(c);
            self.pos += 1;
        }
    }

    fn read_string(&mut self, quote: u8) -> Result<Vec<u8>, LexError> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                return self.err("unfinished string");
            }
            let c = self.src[self.pos];
            if c == quote {
                self.pos += 1;
                return Ok(out);
            }
            match c {
                b'\n' => return self.err("unfinished string"),
                b'\\' => {
                    self.pos += 1;
                    let e = self.peek(0);
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'a' => out.push(7),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'v' => out.push(11),
                        b'\\' => out.push(b'\\'),
                        b'"' => out.push(b'"'),
                        b'\'' => out.push(b'\''),
                        b'\n' => {
                            self.line += 1;
                            out.push(b'\n');
                        }
                        b'x' => {
                            let h = std::str::from_utf8(
                                &self.src[self.pos + 1..(self.pos + 3).min(self.src.len())],
                            )
                            .ok()
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                            match h {
                                Some(v) => {
                                    out.push(v);
                                    self.pos += 2;
                                }
                                None => return self.err("invalid hex escape"),
                            }
                        }
                        b'z' => {
                            self.pos += 1;
                            while matches!(self.peek(0), b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
                            {
                                if self.peek(0) == b'\n' {
                                    self.line += 1;
                                }
                                self.pos += 1;
                            }
                            continue;
                        }
                        d if d.is_ascii_digit() => {
                            let mut v: u32 = 0;
                            let mut n = 0;
                            while n < 3 && self.peek(0).is_ascii_digit() {
                                v = v * 10 + (self.peek(0) - b'0') as u32;
                                self.pos += 1;
                                n += 1;
                            }
                            if v > 255 {
                                return self.err("escape sequence too large");
                            }
                            out.push(v as u8);
                            continue;
                        }
                        _ => return self.err("invalid escape sequence"),
                    }
                    self.pos += 1;
                }
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn read_number(&mut self) -> Result<f64, LexError> {
        let start = self.pos;
        if self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X') {
            self.pos += 2;
            while self.peek(0).is_ascii_hexdigit() {
                self.pos += 1;
            }
        } else {
            loop {
                let c = self.peek(0);
                if c.is_ascii_digit() || c == b'.' {
                    self.pos += 1;
                } else if matches!(c, b'e' | b'E') {
                    self.pos += 1;
                    if matches!(self.peek(0), b'+' | b'-') {
                        self.pos += 1;
                    }
                } else {
                    break;
                }
            }
        }
        // LuaJIT number suffixes (LL, ULL, i) are not used upstream
        let text = &self.src[start..self.pos];
        match crate::str_to_number(text) {
            Some(n) => Ok(n),
            None => self.err(format!(
                "malformed number '{}'",
                String::from_utf8_lossy(text)
            )),
        }
    }

    fn next(&mut self) -> Result<Token, LexError> {
        self.skip_space_and_comments()?;
        let start = self.pos;
        let line = self.line;
        let tok = if self.pos >= self.src.len() {
            Tok::Eof
        } else {
            let c = self.src[self.pos];
            match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    while matches!(self.peek(0), b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9') {
                        self.pos += 1;
                    }
                    let word = std::str::from_utf8(&self.src[start..self.pos]).expect("ascii");
                    keyword(word).unwrap_or_else(|| Tok::Name(word.to_string()))
                }
                b'0'..=b'9' => Tok::Num(self.read_number()?),
                b'.' if self.peek(1).is_ascii_digit() => Tok::Num(self.read_number()?),
                b'"' | b'\'' => Tok::Str(self.read_string(c)?),
                b'[' => match self.long_bracket_level() {
                    Some(level) => Tok::Str(self.read_long_string(level)?),
                    None => {
                        self.pos += 1;
                        Tok::LBracket
                    }
                },
                _ => {
                    let (tok, len) = match (c, self.peek(1), self.peek(2)) {
                        (b'.', b'.', b'.') => (Tok::Dots, 3),
                        (b'.', b'.', _) => (Tok::Concat, 2),
                        (b'=', b'=', _) => (Tok::Eq, 2),
                        (b'~', b'=', _) => (Tok::Ne, 2),
                        (b'<', b'=', _) => (Tok::Le, 2),
                        (b'>', b'=', _) => (Tok::Ge, 2),
                        (b':', b':', _) => (Tok::DoubleColon, 2),
                        (b'+', _, _) => (Tok::Plus, 1),
                        (b'-', _, _) => (Tok::Minus, 1),
                        (b'*', _, _) => (Tok::Star, 1),
                        (b'/', _, _) => (Tok::Slash, 1),
                        (b'%', _, _) => (Tok::Percent, 1),
                        (b'^', _, _) => (Tok::Caret, 1),
                        (b'#', _, _) => (Tok::Hash, 1),
                        (b'<', _, _) => (Tok::Lt, 1),
                        (b'>', _, _) => (Tok::Gt, 1),
                        (b'=', _, _) => (Tok::Assign, 1),
                        (b'(', _, _) => (Tok::LParen, 1),
                        (b')', _, _) => (Tok::RParen, 1),
                        (b'{', _, _) => (Tok::LBrace, 1),
                        (b'}', _, _) => (Tok::RBrace, 1),
                        (b']', _, _) => (Tok::RBracket, 1),
                        (b';', _, _) => (Tok::Semi, 1),
                        (b':', _, _) => (Tok::Colon, 1),
                        (b',', _, _) => (Tok::Comma, 1),
                        (b'.', _, _) => (Tok::Dot, 1),
                        _ => return self.err(format!("unexpected character '{}'", c as char)),
                    };
                    self.pos += len;
                    tok
                }
            }
        };
        Ok(Token {
            tok,
            line,
            start,
            end: self.pos,
        })
    }
}

fn keyword(w: &str) -> Option<Tok> {
    Some(match w {
        "and" => Tok::And,
        "break" => Tok::Break,
        "do" => Tok::Do,
        "else" => Tok::Else,
        "elseif" => Tok::Elseif,
        "end" => Tok::End,
        "false" => Tok::False,
        "for" => Tok::For,
        "function" => Tok::Function,
        "goto" => Tok::Goto,
        "if" => Tok::If,
        "in" => Tok::In,
        "local" => Tok::Local,
        "nil" => Tok::Nil,
        "not" => Tok::Not,
        "or" => Tok::Or,
        "repeat" => Tok::Repeat,
        "return" => Tok::Return,
        "then" => Tok::Then,
        "true" => Tok::True,
        "until" => Tok::Until,
        "while" => Tok::While,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s.as_bytes())
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn basics() {
        assert_eq!(
            toks("local x = { [\"a\"] = 1.5, 'b\\n' } -- c\n--[[ long\n]] x..y ..."),
            vec![
                Tok::Local,
                Tok::Name("x".into()),
                Tok::Assign,
                Tok::LBrace,
                Tok::LBracket,
                Tok::Str(b"a".to_vec()),
                Tok::RBracket,
                Tok::Assign,
                Tok::Num(1.5),
                Tok::Comma,
                Tok::Str(b"b\n".to_vec()),
                Tok::RBrace,
                Tok::Name("x".into()),
                Tok::Concat,
                Tok::Name("y".into()),
                Tok::Dots,
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("[==[a]]b]==]"),
            vec![Tok::Str(b"a]]b".to_vec()), Tok::Eof]
        );
        assert_eq!(toks("0x2FFF0000"), vec![Tok::Num(805240832.0), Tok::Eof]);
        assert_eq!(
            toks("\"\\65\\066\""),
            vec![Tok::Str(b"AB".to_vec()), Tok::Eof]
        );
    }
}
