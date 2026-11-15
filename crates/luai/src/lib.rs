//! The Lua interpreter's parts that only build time and upstream's
//! translated program need: the lexer, the parser and its syntax tree, and
//! the standard library environment (`env`). The values, patterns and
//! formatting they work on are in `lua`, which the typed engine ships.

pub mod ast;
pub mod env;
pub mod lexer;
pub mod parser;
