//! Upstream's Lua program, translated to Rust by
//! `cargo run -p importer --bin luagen`, and a headless host that runs it.
//! The reference implementation the engine is tested against, see
//! docs/design.md.

pub mod host;
pub mod imported;
mod utf8;

/// Upstream's modules, translated; `chunk(path)` finds one.
pub mod up {
    include!(concat!(env!("OUT_DIR"), "/up.rs"));
}

/// Functions of upstream's data files, translated; `make(file, offset, ...)`
/// creates one.
pub mod data {
    include!(concat!(env!("OUT_DIR"), "/data.rs"));
}
