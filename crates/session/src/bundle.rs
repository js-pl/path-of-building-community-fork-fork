//! Data bundles: upstream data files packed into one zlib-compressed blob,
//! which the site serves and the browser hands to the engine.
//!
//! Format (before compression): `PBDB`, u32 file count, then per file a u32
//! path length, the path (relative to upstream's `src/`), a u32 data length
//! and the data. Integers are little endian.

use crate::Files;

const MAGIC: &[u8; 4] = b"PBDB";

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
    miniz_oxide::deflate::compress_to_vec_zlib(&out, 6)
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
