//! The engine's typed game data, made at build time from upstream's data
//! files: the importer evaluates them, [`data_lua`] does what
//! `Modules/Data.lua` does on top of them, and the engine's typed tables
//! are read from the result and encoded in its binary form
//! (`engine::codec`). No Lua runs when the engine loads it.

pub mod data_lua;
mod jsonc;

use engine::game_data::GameData;
use engine::item_data::ItemData;
use lua::{TableRef, Value};
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;

/// Upstream's `data` table as `Data.lua` leaves it (the parts ported so
/// far), with the globals of `Data/Global.lua`
pub struct Source {
    pub globals: TableRef,
    pub data: TableRef,
}

impl Source {
    /// Imports upstream's data files from its `src/` directory and
    /// processes them.
    pub fn load(src_dir: &Path) -> Result<Source> {
        let raw = importer::import(src_dir).map_err(|e| e.to_string())?;
        let src = Source {
            globals: raw.globals,
            data: raw.data,
        };
        data_lua::process(&src, src_dir)?;
        Ok(src)
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    pub fn items(&self) -> Result<ItemData> {
        ItemData::from_lua(&Value::Table(self.data.clone()), &self.global("colorCodes"))
            .map_err(|e| format!("item data: {e}"))
    }

    pub fn game_data(&self) -> Result<GameData> {
        Ok(GameData {
            items: self.items()?,
        })
    }
}

/// The typed game data of upstream's `src/` directory, encoded.
pub fn build(src_dir: &Path) -> Result<Vec<u8>> {
    Ok(engine::codec::encode(&Source::load(src_dir)?.game_data()?))
}
