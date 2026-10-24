//! The typed modifier core: mods, their tags, the stores that hold them and
//! the queries the calculations make. Plain Rust, replacing the translation
//! of upstream's `ModStore`, `ModDB`, `ModList` and `modLib.createMod`.

pub mod convert;
pub mod store;
pub mod sym;
pub mod tag;
pub mod types;

pub use store::{Actors, Cfg, Mods, Query, Row, Store, StoreId, Stores};
pub use sym::Sym;
pub use tag::{Names, Tag, TagKind};
pub use types::{Mod, ModType, ModValue, Record};
