//! Translates upstream's Lua program to Rust, as `crates/upstream`'s build
//! script does, into a directory to read the generated code:
//!
//!   cargo run -p importer --bin luagen [out dir, default target/luagen]

use importer::luagen;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/luagen"));
    let g = luagen::generate(&root, &out).unwrap_or_else(|e| panic!("{e}"));
    for f in &g.files {
        let _ = std::process::Command::new("rustfmt")
            .arg("--edition=2024")
            .arg(out.join(f))
            .status();
    }
    for (file, why) in &g.failed {
        println!("{file}: {why}");
    }
    println!(
        "{} files written to {}, {} failed",
        g.files.len(),
        out.display(),
        g.failed.len()
    );
    if !g.failed.is_empty() {
        std::process::exit(1);
    }
}
