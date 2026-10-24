//! Statistics on function literals inside a file's table constructors.
use lua::ast::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Stats {
    funcs: usize,
    single_return: usize,
    stmt_kinds: BTreeMap<String, usize>,
    calls: BTreeMap<String, usize>,
    methods: BTreeMap<String, usize>,
}

fn expr(e: &Expr, st: &mut Stats, top: bool) {
    match e {
        Expr::Function(f) => {
            if top {
                st.funcs += 1;
                if f.body.stats.is_empty() && matches!(f.body.last, Some(LastStat::Return(_))) {
                    st.single_return += 1;
                }
            }
            block(&f.body, st);
        }
        Expr::Table(fields) => fields.iter().for_each(|f| match f {
            Field::Keyed(k, v) => {
                expr(k, st, top);
                expr(v, st, top);
            }
            Field::Item(v) => expr(v, st, top),
        }),
        Expr::Call { func, args } => {
            if !top {
                let name = match &**func {
                    Expr::Name(n) => n.clone(),
                    Expr::Index(o, k) => format!(
                        "{}.{}",
                        match &**o {
                            Expr::Name(n) => n.clone(),
                            _ => "?".into(),
                        },
                        match &**k {
                            Expr::Str(s) => String::from_utf8_lossy(s).to_string(),
                            _ => "?".into(),
                        }
                    ),
                    _ => "?".into(),
                };
                *st.calls.entry(name).or_default() += 1;
            }
            expr(func, st, top);
            args.iter().for_each(|a| expr(a, st, top));
        }
        Expr::Method { obj, name, args } => {
            if !top {
                *st.methods.entry(name.clone()).or_default() += 1;
            }
            expr(obj, st, top);
            args.iter().for_each(|a| expr(a, st, top));
        }
        Expr::Index(a, b) | Expr::Binary(_, a, b) => {
            expr(a, st, top);
            expr(b, st, top);
        }
        Expr::Unary(_, a) | Expr::Paren(a) => expr(a, st, top),
        _ => {}
    }
}

fn block(b: &Block, st: &mut Stats) {
    for s in &b.stats {
        let k = match s {
            Stat::Local { .. } => "local",
            Stat::Assign { .. } => "assign",
            Stat::Call(_) => "call",
            Stat::If { .. } => "if",
            Stat::NumericFor { .. } => "numfor",
            Stat::GenericFor { .. } => "genfor",
            Stat::While { .. } => "while",
            Stat::LocalFunction { .. } => "localfunc",
            _ => "other",
        };
        *st.stmt_kinds.entry(k.into()).or_default() += 1;
        match s {
            Stat::Local { values, .. } => values.iter().for_each(|e| expr(e, st, false)),
            Stat::Assign { targets, values } => targets
                .iter()
                .chain(values)
                .for_each(|e| expr(e, st, false)),
            Stat::Call(e) => expr(e, st, false),
            Stat::If { conds, otherwise } => {
                for (c, b) in conds {
                    expr(c, st, false);
                    block(b, st);
                }
                if let Some(b) = otherwise {
                    block(b, st);
                }
            }
            Stat::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                expr(start, st, false);
                expr(limit, st, false);
                if let Some(s) = step {
                    expr(s, st, false);
                }
                block(body, st);
            }
            Stat::GenericFor { exprs, body, .. } => {
                exprs.iter().for_each(|e| expr(e, st, false));
                block(body, st);
            }
            Stat::While { cond, body } => {
                expr(cond, st, false);
                block(body, st);
            }
            Stat::LocalFunction { func, .. } => block(&func.body, st),
            _ => {}
        }
    }
    if let Some(LastStat::Return(es)) = &b.last {
        es.iter().for_each(|e| expr(e, st, false));
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read(&path).unwrap();
    let b = lua::parser::parse(&src).unwrap();
    let mut st = Stats::default();
    for s in &b.stats {
        if let Stat::Local { values, .. } = s {
            values.iter().for_each(|e| expr(e, &mut st, true));
        }
    }
    println!(
        "closures in tables: {}, single return: {}",
        st.funcs, st.single_return
    );
    println!("statements: {:?}", st.stmt_kinds);
    println!("calls: {:?}", st.calls);
    println!("methods: {:?}", st.methods);
}
