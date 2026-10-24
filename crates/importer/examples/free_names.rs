//! Lists top-level locals of a Lua file and the names their initialisers
//! reference outside nested functions.
use lua::ast::*;
use std::collections::BTreeSet;

fn walk(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Name(n) => {
            out.insert(n.clone());
        }
        Expr::Table(fields) => {
            for f in fields {
                match f {
                    Field::Keyed(k, v) => {
                        walk(k, out);
                        walk(v, out);
                    }
                    Field::Item(v) => walk(v, out),
                }
            }
        }
        Expr::Index(a, b) | Expr::Binary(_, a, b) => {
            walk(a, out);
            walk(b, out);
        }
        Expr::Call { func, args } => {
            walk(func, out);
            args.iter().for_each(|a| walk(a, out));
        }
        Expr::Method { obj, args, .. } => {
            walk(obj, out);
            args.iter().for_each(|a| walk(a, out));
        }
        Expr::Unary(_, a) | Expr::Paren(a) => walk(a, out),
        _ => {}
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read(&path).unwrap();
    let block = lua::parser::parse(&src).unwrap();
    for (s, line) in block.stats.iter().zip(&block.lines) {
        let (kind, names, exprs): (&str, Vec<String>, Vec<&Expr>) = match s {
            Stat::Local { names, values } => ("local", names.clone(), values.iter().collect()),
            Stat::LocalFunction { name, .. } => ("local function", vec![name.clone()], vec![]),
            Stat::Assign { targets, values } => (
                "assign",
                targets
                    .iter()
                    .map(|t| format!("{t:?}").chars().take(60).collect())
                    .collect(),
                values.iter().collect(),
            ),
            Stat::Function { path, .. } => ("function", path.clone(), vec![]),
            Stat::Call(_) => ("call", vec![], vec![]),
            Stat::NumericFor { .. } | Stat::GenericFor { .. } => ("for", vec![], vec![]),
            Stat::If { .. } => ("if", vec![], vec![]),
            Stat::Do(_) => ("do", vec![], vec![]),
            _ => ("other", vec![], vec![]),
        };
        let mut free = BTreeSet::new();
        for e in exprs {
            walk(e, &mut free);
        }
        println!(
            "{line}: {kind} {} <- {}",
            names.join(","),
            free.into_iter().collect::<Vec<_>>().join(" ")
        );
    }
}
