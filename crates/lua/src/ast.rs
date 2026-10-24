//! Lua syntax tree.

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stats: Vec<Stat>,
    /// Line each statement starts on
    pub lines: Vec<u32>,
    /// `return` or `break` at the end of the block
    pub last: Option<LastStat>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LastStat {
    Return(Vec<Expr>),
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stat {
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    Call(Expr),
    Do(Block),
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: Expr,
    },
    If {
        conds: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    NumericFor {
        var: String,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        vars: Vec<String>,
        exprs: Vec<Expr>,
        body: Block,
    },
    /// `function a.b:c() end`; `path` is `a`, `b`, `method` is `c`
    Function {
        path: Vec<String>,
        method: Option<String>,
        func: FuncBody,
    },
    LocalFunction {
        name: String,
        func: FuncBody,
    },
    Local {
        names: Vec<String>,
        values: Vec<Expr>,
    },
    Goto(String),
    Label(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncBody {
    pub params: Vec<String>,
    pub vararg: bool,
    pub body: Block,
    /// Line of the `function` keyword
    pub line: u32,
    /// Byte range of the whole function in the source, from `function` to `end`
    pub span: (usize, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Nil,
    True,
    False,
    Number(f64),
    Str(Vec<u8>),
    Vararg,
    Function(Box<FuncBody>),
    Table(Vec<Field>),
    Name(String),
    Index(Box<Expr>, Box<Expr>),
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    Method {
        obj: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    /// `(expr)`, which truncates multiple results to one
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// `[key] = value` or `name = value`
    Keyed(Expr, Expr),
    /// Positional entry
    Item(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
}

/// The value of an expression LuaJIT's parser folds to a constant: literals,
/// and `-`, `not` and arithmetic on constants, except where folding would
/// give NaN or -0 (lj_parse.c `foldarith`, `bcemit_unop`).
pub fn constant(e: &Expr) -> Option<Constant> {
    match e {
        Expr::Nil => Some(Constant::Nil),
        Expr::True => Some(Constant::Bool(true)),
        Expr::False => Some(Constant::Bool(false)),
        Expr::Number(n) => Some(Constant::Number(*n)),
        Expr::Str(_) => Some(Constant::Str),
        Expr::Paren(e) => constant(e),
        Expr::Unary(UnOp::Neg, e) => match constant(e)? {
            Constant::Number(n) if n != 0.0 => Some(Constant::Number(-n)),
            _ => None,
        },
        Expr::Unary(UnOp::Not, e) => match constant(e)? {
            Constant::Nil | Constant::Bool(false) => Some(Constant::Bool(true)),
            _ => Some(Constant::Bool(false)),
        },
        Expr::Binary(op, a, b) => {
            let (Constant::Number(x), Constant::Number(y)) = (constant(a)?, constant(b)?) else {
                return None;
            };
            let n = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
                BinOp::Mod => x - (x / y).floor() * y,
                BinOp::Pow => x.powf(y),
                _ => return None,
            };
            if n.is_nan() || (n == 0.0 && n.is_sign_negative()) {
                None
            } else {
                Some(Constant::Number(n))
            }
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constant {
    Nil,
    Bool(bool),
    Number(f64),
    Str,
}

/// The order in which a table constructor's fields are stored, as LuaJIT
/// does it: fields with a constant key and value go into a template table
/// the constructor copies, the others are stored afterwards in source order.
/// This only matters for duplicate keys: `{ a = f(), a = 1 }` keeps `f()`.
pub fn constructor_order(fields: &[Field]) -> Vec<usize> {
    let template = |f: &Field| match f {
        Field::Keyed(k, v) => {
            !matches!(constant(k), None | Some(Constant::Nil)) && constant(v).is_some()
        }
        Field::Item(v) => constant(v).is_some(),
    };
    let mut order: Vec<usize> = (0..fields.len())
        .filter(|&i| template(&fields[i]))
        .collect();
    order.extend((0..fields.len()).filter(|&i| !template(&fields[i])));
    order
}
