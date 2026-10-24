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
