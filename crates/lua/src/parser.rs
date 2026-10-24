//! Recursive descent parser for Lua 5.1 (plus `goto`), following `lparser.c`.

use crate::ast::*;
use crate::lexer::{LexError, Tok, Token, tokenize};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: u32,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            line: e.line,
            msg: e.msg,
        }
    }
}

type Result<T> = std::result::Result<T, ParseError>;

pub fn parse(src: &[u8]) -> Result<Block> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    let block = p.block()?;
    if p.peek() != &Tok::Eof {
        return p.err(format!("'<eof>' expected near {:?}", p.peek()));
    }
    Ok(block)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

const UNARY_PRIORITY: u8 = 8;

fn binop(t: &Tok) -> Option<(BinOp, u8, u8)> {
    Some(match t {
        Tok::Plus => (BinOp::Add, 6, 6),
        Tok::Minus => (BinOp::Sub, 6, 6),
        Tok::Star => (BinOp::Mul, 7, 7),
        Tok::Slash => (BinOp::Div, 7, 7),
        Tok::Percent => (BinOp::Mod, 7, 7),
        Tok::Caret => (BinOp::Pow, 10, 9),
        Tok::Concat => (BinOp::Concat, 5, 4),
        Tok::Eq => (BinOp::Eq, 3, 3),
        Tok::Ne => (BinOp::Ne, 3, 3),
        Tok::Lt => (BinOp::Lt, 3, 3),
        Tok::Le => (BinOp::Le, 3, 3),
        Tok::Gt => (BinOp::Gt, 3, 3),
        Tok::Ge => (BinOp::Ge, 3, 3),
        Tok::And => (BinOp::And, 2, 2),
        Tok::Or => (BinOp::Or, 1, 1),
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, off: usize) -> &Tok {
        &self.toks[(self.pos + off).min(self.toks.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn advance(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(ParseError {
            line: self.line(),
            msg: msg.into(),
        })
    }

    fn check(&mut self, t: Tok) -> bool {
        if *self.peek() == t {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: Tok) -> Result<Token> {
        if *self.peek() == t {
            Ok(self.advance())
        } else {
            self.err(format!("{t:?} expected near {:?}", self.peek()))
        }
    }

    fn name(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Name(n) => {
                self.advance();
                Ok(n)
            }
            t => self.err(format!("name expected near {t:?}")),
        }
    }

    fn block_follow(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Else | Tok::Elseif | Tok::End | Tok::Until | Tok::Eof
        )
    }

    fn block(&mut self) -> Result<Block> {
        let mut stats = Vec::new();
        let mut lines = Vec::new();
        let mut last = None;
        while !self.block_follow() {
            match self.peek() {
                Tok::Return => {
                    self.advance();
                    let exprs = if self.block_follow() || *self.peek() == Tok::Semi {
                        vec![]
                    } else {
                        self.exprlist()?
                    };
                    self.check(Tok::Semi);
                    last = Some(LastStat::Return(exprs));
                    break;
                }
                Tok::Break => {
                    self.advance();
                    self.check(Tok::Semi);
                    // Lua 5.1 requires break to end a block; LuaJIT allows more after it
                    if self.block_follow() {
                        last = Some(LastStat::Break);
                        break;
                    }
                    lines.push(self.line());
                    stats.push(Stat::Goto("break".into()));
                    continue;
                }
                Tok::Semi => {
                    self.advance();
                    continue;
                }
                _ => {}
            }
            lines.push(self.line());
            stats.push(self.statement()?);
        }
        Ok(Block { stats, lines, last })
    }

    fn statement(&mut self) -> Result<Stat> {
        match self.peek().clone() {
            Tok::If => self.if_stat(),
            Tok::While => {
                self.advance();
                let cond = self.expr()?;
                self.expect(Tok::Do)?;
                let body = self.block()?;
                self.expect(Tok::End)?;
                Ok(Stat::While { cond, body })
            }
            Tok::Do => {
                self.advance();
                let b = self.block()?;
                self.expect(Tok::End)?;
                Ok(Stat::Do(b))
            }
            Tok::For => self.for_stat(),
            Tok::Repeat => {
                self.advance();
                let body = self.block()?;
                self.expect(Tok::Until)?;
                let cond = self.expr()?;
                Ok(Stat::Repeat { body, cond })
            }
            Tok::Function => {
                let start = self.toks[self.pos].start;
                let line = self.line();
                self.advance();
                let mut path = vec![self.name()?];
                let mut method = None;
                while self.check(Tok::Dot) {
                    path.push(self.name()?);
                }
                if self.check(Tok::Colon) {
                    method = Some(self.name()?);
                }
                let func = self.func_body(method.is_some(), line, start)?;
                Ok(Stat::Function { path, method, func })
            }
            Tok::Local => {
                self.advance();
                if *self.peek() == Tok::Function {
                    let start = self.toks[self.pos].start;
                    let line = self.line();
                    self.advance();
                    let name = self.name()?;
                    let func = self.func_body(false, line, start)?;
                    return Ok(Stat::LocalFunction { name, func });
                }
                let mut names = vec![self.name()?];
                while self.check(Tok::Comma) {
                    names.push(self.name()?);
                }
                let values = if self.check(Tok::Assign) {
                    self.exprlist()?
                } else {
                    vec![]
                };
                Ok(Stat::Local { names, values })
            }
            Tok::DoubleColon => {
                self.advance();
                let n = self.name()?;
                self.expect(Tok::DoubleColon)?;
                Ok(Stat::Label(n))
            }
            Tok::Goto => {
                self.advance();
                Ok(Stat::Goto(self.name()?))
            }
            _ => self.expr_stat(),
        }
    }

    fn if_stat(&mut self) -> Result<Stat> {
        self.advance();
        let mut conds = Vec::new();
        let cond = self.expr()?;
        self.expect(Tok::Then)?;
        conds.push((cond, self.block()?));
        let mut otherwise = None;
        loop {
            match self.peek() {
                Tok::Elseif => {
                    self.advance();
                    let c = self.expr()?;
                    self.expect(Tok::Then)?;
                    conds.push((c, self.block()?));
                }
                Tok::Else => {
                    self.advance();
                    otherwise = Some(self.block()?);
                    self.expect(Tok::End)?;
                    break;
                }
                _ => {
                    self.expect(Tok::End)?;
                    break;
                }
            }
        }
        Ok(Stat::If { conds, otherwise })
    }

    fn for_stat(&mut self) -> Result<Stat> {
        self.advance();
        let first = self.name()?;
        if self.check(Tok::Assign) {
            let start = self.expr()?;
            self.expect(Tok::Comma)?;
            let limit = self.expr()?;
            let step = if self.check(Tok::Comma) {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(Tok::Do)?;
            let body = self.block()?;
            self.expect(Tok::End)?;
            return Ok(Stat::NumericFor {
                var: first,
                start,
                limit,
                step,
                body,
            });
        }
        let mut vars = vec![first];
        while self.check(Tok::Comma) {
            vars.push(self.name()?);
        }
        self.expect(Tok::In)?;
        let exprs = self.exprlist()?;
        self.expect(Tok::Do)?;
        let body = self.block()?;
        self.expect(Tok::End)?;
        Ok(Stat::GenericFor { vars, exprs, body })
    }

    fn expr_stat(&mut self) -> Result<Stat> {
        let e = self.suffixed_expr()?;
        if matches!(self.peek(), Tok::Assign | Tok::Comma) {
            let mut targets = vec![e];
            while self.check(Tok::Comma) {
                targets.push(self.suffixed_expr()?);
            }
            self.expect(Tok::Assign)?;
            let values = self.exprlist()?;
            for t in &targets {
                if !matches!(t, Expr::Name(_) | Expr::Index(..)) {
                    return self.err("syntax error: cannot assign");
                }
            }
            return Ok(Stat::Assign { targets, values });
        }
        match e {
            Expr::Call { .. } | Expr::Method { .. } => Ok(Stat::Call(e)),
            _ => self.err("syntax error: expression is not a statement"),
        }
    }

    fn func_body(&mut self, is_method: bool, line: u32, start: usize) -> Result<FuncBody> {
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        if is_method {
            params.push("self".to_string());
        }
        let mut vararg = false;
        if *self.peek() != Tok::RParen {
            loop {
                match self.peek().clone() {
                    Tok::Name(n) => {
                        self.advance();
                        params.push(n);
                    }
                    Tok::Dots => {
                        self.advance();
                        vararg = true;
                        break;
                    }
                    t => return self.err(format!("<name> expected near {t:?}")),
                }
                if !self.check(Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(Tok::RParen)?;
        let body = self.block()?;
        let end = self.expect(Tok::End)?;
        Ok(FuncBody {
            params,
            vararg,
            body,
            line,
            span: (start, end.end),
        })
    }

    fn exprlist(&mut self) -> Result<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.check(Tok::Comma) {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub fn expr(&mut self) -> Result<Expr> {
        self.subexpr(0)
    }

    fn subexpr(&mut self, limit: u8) -> Result<Expr> {
        let unop = match self.peek() {
            Tok::Not => Some(UnOp::Not),
            Tok::Minus => Some(UnOp::Neg),
            Tok::Hash => Some(UnOp::Len),
            _ => None,
        };
        let mut left = if let Some(op) = unop {
            self.advance();
            let e = self.subexpr(UNARY_PRIORITY)?;
            match (op, e) {
                // Fold negative literals, as Lua's code generator does
                (UnOp::Neg, Expr::Number(n)) => Expr::Number(-n),
                (op, e) => Expr::Unary(op, Box::new(e)),
            }
        } else {
            self.simple_expr()?
        };
        while let Some((op, lp, rp)) = binop(self.peek()) {
            if lp <= limit {
                break;
            }
            self.advance();
            let right = self.subexpr(rp)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn simple_expr(&mut self) -> Result<Expr> {
        let e = match self.peek().clone() {
            Tok::Num(n) => Expr::Number(n),
            Tok::Str(s) => Expr::Str(s),
            Tok::Nil => Expr::Nil,
            Tok::True => Expr::True,
            Tok::False => Expr::False,
            Tok::Dots => Expr::Vararg,
            Tok::LBrace => return self.table(),
            Tok::Function => {
                let start = self.toks[self.pos].start;
                let line = self.line();
                self.advance();
                return Ok(Expr::Function(Box::new(
                    self.func_body(false, line, start)?,
                )));
            }
            _ => return self.suffixed_expr(),
        };
        self.advance();
        Ok(e)
    }

    fn primary_expr(&mut self) -> Result<Expr> {
        match self.peek().clone() {
            Tok::Name(n) => {
                self.advance();
                Ok(Expr::Name(n))
            }
            Tok::LParen => {
                self.advance();
                let e = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            t => self.err(format!("unexpected symbol near {t:?}")),
        }
    }

    fn suffixed_expr(&mut self) -> Result<Expr> {
        let mut e = self.primary_expr()?;
        loop {
            match self.peek().clone() {
                Tok::Dot => {
                    self.advance();
                    let n = self.name()?;
                    e = Expr::Index(Box::new(e), Box::new(Expr::Str(n.into_bytes())));
                }
                Tok::LBracket => {
                    self.advance();
                    let k = self.expr()?;
                    self.expect(Tok::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(k));
                }
                Tok::Colon => {
                    self.advance();
                    let name = self.name()?;
                    let args = self.call_args()?;
                    e = Expr::Method {
                        obj: Box::new(e),
                        name,
                        args,
                    };
                }
                Tok::LParen | Tok::Str(_) | Tok::LBrace => {
                    let args = self.call_args()?;
                    e = Expr::Call {
                        func: Box::new(e),
                        args,
                    };
                }
                _ => return Ok(e),
            }
        }
    }

    fn call_args(&mut self) -> Result<Vec<Expr>> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.advance();
                Ok(vec![Expr::Str(s)])
            }
            Tok::LBrace => Ok(vec![self.table()?]),
            Tok::LParen => {
                self.advance();
                if self.check(Tok::RParen) {
                    return Ok(vec![]);
                }
                let args = self.exprlist()?;
                self.expect(Tok::RParen)?;
                Ok(args)
            }
            t => self.err(format!("function arguments expected near {t:?}")),
        }
    }

    fn table(&mut self) -> Result<Expr> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        while *self.peek() != Tok::RBrace {
            match self.peek().clone() {
                Tok::LBracket => {
                    self.advance();
                    let k = self.expr()?;
                    self.expect(Tok::RBracket)?;
                    self.expect(Tok::Assign)?;
                    let v = self.expr()?;
                    fields.push(Field::Keyed(k, v));
                }
                Tok::Name(n) if *self.peek_at(1) == Tok::Assign => {
                    self.advance();
                    self.advance();
                    let v = self.expr()?;
                    fields.push(Field::Keyed(Expr::Str(n.into_bytes()), v));
                }
                _ => fields.push(Field::Item(self.expr()?)),
            }
            if !self.check(Tok::Comma) && !self.check(Tok::Semi) {
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        Ok(Expr::Table(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        let b = parse(b"return 1 + 2 * 3 ^ 2 ^ 1, a .. b .. c, not x == y, -2 ^ 2").unwrap();
        let Some(LastStat::Return(e)) = b.last else {
            panic!()
        };
        assert_eq!(
            e[0],
            Expr::Binary(
                BinOp::Add,
                Box::new(Expr::Number(1.0)),
                Box::new(Expr::Binary(
                    BinOp::Mul,
                    Box::new(Expr::Number(2.0)),
                    Box::new(Expr::Binary(
                        BinOp::Pow,
                        Box::new(Expr::Number(3.0)),
                        Box::new(Expr::Binary(
                            BinOp::Pow,
                            Box::new(Expr::Number(2.0)),
                            Box::new(Expr::Number(1.0))
                        ))
                    ))
                ))
            )
        );
        assert!(
            matches!(&e[1], Expr::Binary(BinOp::Concat, _, r) if matches!(**r, Expr::Binary(BinOp::Concat, _, _)))
        );
        assert!(
            matches!(&e[2], Expr::Binary(BinOp::Eq, l, _) if matches!(**l, Expr::Unary(UnOp::Not, _)))
        );
        // -2 ^ 2 is -(2 ^ 2)
        assert!(matches!(&e[3], Expr::Unary(UnOp::Neg, _)));
    }

    #[test]
    fn statements() {
        let src = br#"
local a, b = ...
function m.f:g(x, ...) return x end
for i = 1, 10, 2 do if i then break end end
for k, v in pairs(t) do goto continue ::continue:: end
t["x"].y = { 1, 2; z = 3, [4] = function() end }
f"str" g{1}
repeat local x = 1 until x
"#;
        let b = parse(src).unwrap();
        assert_eq!(b.stats.len(), 8);
        let Stat::Function { path, method, func } = &b.stats[1] else {
            panic!()
        };
        assert_eq!(path, &vec!["m".to_string(), "f".to_string()]);
        assert_eq!(method.as_deref(), Some("g"));
        assert_eq!(func.params, vec!["self".to_string(), "x".to_string()]);
        assert!(func.vararg);
        assert_eq!(
            &src[func.span.0..func.span.1],
            b"function m.f:g(x, ...) return x end"
        );
    }
}
