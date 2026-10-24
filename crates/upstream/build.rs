//! Translates upstream's Lua program to Rust, see `importer::luagen`.

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let g = importer::luagen::generate(&root, &out).unwrap_or_else(|e| panic!("luagen: {e}"));
    if !g.failed.is_empty() {
        let list: Vec<String> = g
            .failed
            .iter()
            .map(|(f, why)| format!("{f}: {why}"))
            .collect();
        panic!("luagen could not translate:\n{}", list.join("\n"));
    }
    for dir in [
        "upstream/PathOfBuilding/src",
        "upstream/PathOfBuilding/runtime/lua",
        "oracle/json.lua",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(dir).display());
    }
}
