//! Data bundles: data files packed into one zlib-compressed blob, which
//! the site serves and the browser hands to the engine.
//!
//! The typed engine's data is in `engine::codec`'s binary form, made by
//! `crates/gamedata` at build time: the game data ([`GAME_DATA`]), the
//! mod parser ([`MOD_PARSER`]) and one passive tree per version
//! ([`tree_path`], each compressed on its own, as older versions are
//! served as separate files). A bundle for the translation also holds
//! upstream's Lua data files, by their path relative to upstream's `src/`.
//!
//! Format (before compression): `PBDB`, u32 file count, then per file a u32
//! path length, the path, a u32 data length and the data. Integers are
//! little endian.

use crate::Files;
use engine::tree::PassiveTree;

const MAGIC: &[u8; 4] = b"PBDB";

/// The typed game data (`engine::game_data::GameData`)
pub const GAME_DATA: &str = "typed/game.bin";
/// The mod parser's tables and upstream's mod cache
/// (`engine::mod_parser::ParserData`)
pub const MOD_PARSER: &str = "typed/modparser.bin";

/// A tree version's passive tree
pub fn tree_path(version: &str) -> String {
    format!("typed/trees/{version}.bin")
}

/// A passive tree as stored in a bundle: its binary form, compressed
pub fn encode_tree(tree: &PassiveTree) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(&tree.encode(), 9)
}

pub fn decode_tree(bytes: &[u8]) -> Result<PassiveTree, String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(bytes)
        .map_err(|e| format!("not a zlib stream: {e:?}"))?;
    PassiveTree::decode(&raw)
}

/// Packs files into a compressed bundle.
pub fn pack<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<u8> {
    let files: Vec<_> = files.into_iter().collect();
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, data) in files {
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    miniz_oxide::deflate::compress_to_vec_zlib(&out, 9)
}
/// Unpacks a bundle into `files`; returns the number of files.
pub fn unpack(bundle: &[u8], files: &Files) -> Result<usize, String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(bundle)
        .map_err(|e| format!("bundle is not a zlib stream: {e:?}"))?;
    let mut pos = 0;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let s = raw.get(pos..pos + n).ok_or("bundle is truncated")?;
        pos += n;
        Ok(s)
    };
    if take(4)? != MAGIC {
        return Err("not a data bundle".into());
    }
    let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let count = u32_at(take(4)?);
    for _ in 0..count {
        let n = u32_at(take(4)?);
        let path = String::from_utf8(take(n)?.to_vec()).map_err(|_| "bad path in bundle")?;
        let n = u32_at(take(4)?);
        files.insert(path, take(n)?.to_vec());
    }
    Ok(count)
}
