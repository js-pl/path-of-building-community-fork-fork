//! Times the typed engine's calculations on corpus builds against the
//! translation's, as a table: `cargo run --release -p equivalence --bin
//! bench -- [--all] [--filter <id>] [--reps N] [--power-reps N]
//! [--power-depth N (0: every node)] [--overrides N] [--typed-only] [--no-power]`

use equivalence::bench::{Options, run, table};

fn main() {
    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);
    let num = |v: Option<String>| -> usize {
        v.and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("expected a number"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => opts.all = true,
            "--filter" => opts.filter = args.next().expect("--filter <id>"),
            "--reps" => opts.reps = num(args.next()),
            "--power-reps" => opts.power_reps = num(args.next()),
            "--power-depth" => {
                opts.power_depth = Some(num(args.next()) as f64).filter(|&d| d > 0.0)
            }
            "--overrides" => opts.overrides = num(args.next()),
            "--typed-only" => opts.translation = false,
            "--no-power" => opts.power = false,
            _ => panic!("unknown argument {a}"),
        }
    }
    println!("{}", table(&run(&opts)));
}
