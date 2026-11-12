//! The engine's typed game data, made at build time from upstream's data
//! files: the importer evaluates them, [`data_lua`] does what
//! `Modules/Data.lua` does on top of them, and the engine's typed tables
//! are read from the result and encoded in its binary form
//! (`engine::codec`). No Lua runs when the engine loads it.

pub mod data_lua;
mod jsonc;

use engine::calc_setup::SetupData;
use engine::config::ConfigData;
use engine::game_data::GameData;
use engine::item_data::ItemData;
use engine::skill_data::SkillData;
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
        engine::data::Data::new(src.globals.clone(), src.data.clone());
        Ok(src)
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    pub fn items(&self) -> Result<ItemData> {
        ItemData::from_lua(&self.value(), &self.global("colorCodes"))
            .map_err(|e| format!("item data: {e}"))
    }

    pub fn skills(&self) -> Result<SkillData> {
        SkillData::from_lua(
            &self.value(),
            &self.global("colorCodes"),
            &self.global("SkillType"),
        )
        .map_err(|e| format!("skill data: {e}"))
    }

    pub fn setup(&self) -> Result<SetupData> {
        SetupData::from_lua(
            &self.value(),
            &self.global("ModFlag"),
            &self.global("KeywordFlag"),
        )
        .map_err(|e| format!("setup data: {e}"))
    }

    pub fn config(&self) -> Result<ConfigData> {
        ConfigData::from_lua(&self.value(), &self.global("SkillType"))
            .map_err(|e| format!("config data: {e}"))
    }

    fn value(&self) -> Value {
        Value::Table(self.data.clone())
    }

    pub fn game_data(&self) -> Result<GameData> {
        Ok(GameData {
            items: self.items()?,
            skills: self.skills()?,
            setup: self.setup()?,
            config: self.config()?,
        })
    }
}

/// The typed game data of upstream's `src/` directory, encoded.
pub fn build(src_dir: &Path) -> Result<Vec<u8>> {
    Ok(engine::codec::encode(&Source::load(src_dir)?.game_data()?))
}
