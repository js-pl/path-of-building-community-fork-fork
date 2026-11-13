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

/// The timeless jewels' data but their look-up tables
/// (`engine::timeless::Timeless::encode`), compressed
pub const TIMELESS: &str = "typed/timeless.bin";

/// A timeless jewel's look-up table by its name without spaces
/// (`LethalPride`): upstream's zlib stream, its parts joined
pub fn timeless_lut_path(name: &str) -> String {
    format!("typed/timeless/{name}.bin")
}

/// The timeless jewels' data as stored in a bundle
pub fn encode_timeless(timeless: &engine::timeless::Timeless) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(&timeless.encode(), 9)
}

/// The timeless jewels' data from a bundle's files; each look-up table is
/// read from them when a jewel first needs it, and dropped from `files`
pub fn decode_timeless(files: &Files) -> Result<engine::timeless::Timeless, String> {
    let bytes = files
        .get(TIMELESS)
        .ok_or_else(|| format!("missing data file {TIMELESS}"))?;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes)
        .map_err(|e| format!("{TIMELESS}: not a zlib stream: {e:?}"))?;
    let luts = files.clone();
    let loader = move |name: &str| {
        let path = timeless_lut_path(name);
        let packed = luts.get(&path)?;
        let lut = miniz_oxide::inflate::decompress_to_vec_zlib(&packed).ok();
        luts.remove(&path);
        lut
    };
    engine::timeless::Timeless::decode(&raw, Box::new(loader))
        .map_err(|e| format!("{TIMELESS}: {e}"))
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

/// Data files by bundle path, with their bytes
pub type FileList = Vec<(String, Vec<u8>)>;

/// The typed engine's data made from upstream's source directory, by
/// bundle path: what starts the engine (game data, mod parser, the latest
/// tree), and what builds need on demand (every tree version, the
/// timeless jewels' data and look-up tables)
#[cfg(feature = "source")]
pub fn typed_files(src_dir: &std::path::Path) -> Result<(FileList, FileList), String> {
    let src = gamedata::Source::load(src_dir)?;
    let game = src.game_data()?;
    let parser_bytes = src.mod_parser_data()?.encode();
    let parser = engine::mod_parser::ModParser::decode(&parser_bytes)?;
    let latest = game.latest_tree_version().to_string();
    let mut core = vec![
        (GAME_DATA.to_string(), game.encode()),
        (MOD_PARSER.to_string(), parser_bytes),
    ];
    let mut lazy = Vec::new();
    for v in &game.tree_versions {
        let tree = src
            .tree(v, &parser, *v == latest)
            .map_err(|e| format!("tree {v}: {e}"))?;
        let file = (tree_path(v), encode_tree(&tree));
        if *v == latest {
            core.push(file.clone());
        }
        lazy.push(file);
    }
    lazy.push((
        TIMELESS.to_string(),
        encode_timeless(&src.timeless(&parser)?),
    ));
    for ty in engine::timeless::TYPES {
        let name = ty.replace(' ', "");
        let packed = gamedata::timeless_lut_packed(src_dir, &name)
            .ok_or_else(|| format!("no look-up table for {ty}"))?;
        lazy.push((timeless_lut_path(&name), packed));
    }
    Ok((core, lazy))
}
