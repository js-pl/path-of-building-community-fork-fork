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

/// Whether an expression can give several values.
pub fn is_multi(e: &Expr) -> bool {
    matches!(e, Expr::Call { .. } | Expr::Method { .. } | Expr::Vararg)
}

/// How LuaJIT's parser builds a table constructor (`expr_table`): fields
/// with a constant key and value go into a template table the constructor
/// copies, the others are stored afterwards in source order. The plan
/// reproduces the template's layout, so number keys are placed as LuaJIT
/// places them, and the values duplicate keys end up with:
/// `{ a = f(), a = 1 }` keeps `f()`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstructorPlan {
    /// Array and hash sizes of the table made before any field is stored
    pub asize: u32,
    pub hbits: u32,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Store field `i` in the template
    Template(usize),
    /// Store field `i` after the template
    Field(usize),
    /// A template slot for a string key whose value is stored later or nil
    Reserve,
    /// Grow the array part for keys up to `n` (`lj_tab_reasize`)
    Reasize(u32),
}

pub fn constructor_plan(fields: &[Field]) -> ConstructorPlan {
    let (mut narr, mut nhash, mut needarr) = (1u32, 0u32, false);
    let mut template: Option<(u32, u32)> = None;
    let mut steps = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let step = match f {
            Field::Keyed(k, v) => {
                let (k, v) = (constant(k), constant(v));
                if k == Some(Constant::Number(0.0)) {
                    needarr = true;
                } else {
                    nhash += 1;
                }
                match (k, v) {
                    (None | Some(Constant::Nil), _) => None,
                    (Some(Constant::Str), None | Some(Constant::Nil)) => Some(Step::Reserve),
                    (Some(_), Some(_)) => Some(Step::Template(i)),
                    (Some(_), None) => None,
                }
            }
            Field::Item(v) => {
                narr += 1;
                needarr = true;
                constant(v).map(|_| Step::Template(i))
            }
        };
        if let Some(step) = step {
            template.get_or_insert((if needarr { narr } else { 0 }, crate::ljtab::hbits(nhash)));
            steps.push(step);
        }
    }
    let (asize, hbits) = match template {
        Some(sizes) => {
            if needarr {
                steps.push(Step::Reasize(narr - 1));
            }
            sizes
        }
        None if needarr => (narr.clamp(3, 0x7ff), crate::ljtab::hbits(nhash)),
        None => (0, crate::ljtab::hbits(nhash)),
    };
    let templated: Vec<bool> = {
        let mut t = vec![false; fields.len()];
        for s in &steps {
            if let Step::Template(i) = s {
                t[*i] = true;
            }
        }
        t
    };
    steps.extend(
        (0..fields.len())
            .filter(|&i| !templated[i])
            .map(Step::Field),
    );
    ConstructorPlan {
        asize,
        hbits,
        steps,
    }
}

/// The function of a data file written as `return function(params) ... end`
pub fn data_wrapper(b: &Block) -> Option<&FuncBody> {
    match (&b.stats[..], &b.last) {
        ([], Some(LastStat::Return(es))) => match &es[..] {
            [Expr::Function(f)] => Some(f),
            _ => None,
        },
        _ => None,
    }
}

/// The body of a statement `(function() ... end)()`, which data files use to
/// split long chunks
pub fn iife_body(s: &Stat) -> Option<&Block> {
    let Stat::Call(Expr::Call { func, args }) = s else {
        return None;
    };
    match &**func {
        Expr::Paren(f) => match &**f {
            Expr::Function(fb) if args.is_empty() && fb.params.is_empty() && !fb.vararg => {
                Some(&fb.body)
            }
            _ => None,
        },
        _ => None,
    }
}
