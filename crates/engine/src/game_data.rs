//! The game data the typed engine reads, as the `gamedata` crate makes it
//! from upstream's data files at build time, in the binary form of
//! [`crate::codec`].

use crate::codec;
use crate::item_data::ItemData;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameData {
    pub items: ItemData,
}

crate::codec_struct!(GameData { items });

impl GameData {
    /// Reads the data from its binary form.
    pub fn decode(bytes: &[u8]) -> codec::Result<GameData> {
        codec::decode(bytes)
    }

    pub fn encode(&self) -> Vec<u8> {
        codec::encode(self)
    }
}
