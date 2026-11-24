//! WebSocket (RFC 6455), as much as the web app's connection needs: the
//! handshake's accept key, and text, ping, pong and close frames, with
//! fragmented messages put together. Messages are capped like control
//! socket lines.

use crate::control::MAX_LINE;
use std::io::{Read, Write};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The `Sec-WebSocket-Accept` for a client's `Sec-WebSocket-Key`
pub fn accept_key(key: &str) -> String {
    let mut input = key.trim().as_bytes().to_vec();
    input.extend_from_slice(GUID.as_bytes());
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);
    base64(d.as_ref())
}

/// Standard base64, padded
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A message read from the client
#[derive(Debug, PartialEq)]
pub enum Message {
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// A frame the host sends
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Text(String),
    Pong(Vec<u8>),
    Close,
}

fn frame(
    w: &mut impl Write,
    opcode: u8,
    payload: &[u8],
    mask: Option<[u8; 4]>,
) -> std::io::Result<()> {
    let mut head = vec![0x80 | opcode];
    let m = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => head.push(m | n as u8),
        n if n <= 0xffff => {
            head.push(m | 126);
            head.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            head.push(m | 127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(k) => {
            head.extend_from_slice(&k);
            head.extend(payload.iter().enumerate().map(|(i, b)| b ^ k[i % 4]));
            w.write_all(&head)?;
        }
        None => {
            w.write_all(&head)?;
            w.write_all(payload)?;
        }
    }
    w.flush()
}

/// Writes a frame from the host (unmasked)
pub fn write(w: &mut impl Write, out: &Out) -> std::io::Result<()> {
    match out {
        Out::Text(t) => frame(w, 0x1, t.as_bytes(), None),
        Out::Pong(p) => frame(w, 0xA, p, None),
        Out::Close => frame(w, 0x8, &[], None),
    }
}

/// Writes a text frame as a client does, masked (tests and tools)
pub fn write_client(w: &mut impl Write, text: &str) -> std::io::Result<()> {
    let k = crate::tokens::random(4).map_err(std::io::Error::other)?;
    frame(w, 0x1, text.as_bytes(), Some([k[0], k[1], k[2], k[3]]))
}

/// Reads one frame: fin, opcode, payload (unmasked)
fn read_frame(r: &mut impl Read, need_mask: bool) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let fin = h[0] & 0x80 != 0;
    let opcode = h[0] & 0x0f;
    let masked = h[1] & 0x80 != 0;
    if need_mask && !masked {
        return Err(std::io::Error::other("a client's frame is masked"));
    }
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            u16::from_be_bytes(b) as u64
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            u64::from_be_bytes(b)
        }
        n => n as u64,
    };
    if len > MAX_LINE as u64 {
        return Err(std::io::Error::other("a message over the size limit"));
    }
    let mut key = [0u8; 4];
    if masked {
        r.read_exact(&mut key)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }
    Ok((fin, opcode, payload))
}

/// Reads the next message; fragments are put together, up to the cap.
/// `client` reads the host's frames (unmasked) instead of a client's.
pub fn read(r: &mut impl Read, client: bool) -> std::io::Result<Message> {
    let mut text: Option<Vec<u8>> = None;
    loop {
        let (fin, opcode, payload) = read_frame(r, !client)?;
        match opcode {
            0x1 | 0x2 if text.is_none() => text = Some(payload),
            0x0 if text.is_some() => {
                let t = text.as_mut().unwrap();
                if t.len() + payload.len() > MAX_LINE {
                    return Err(std::io::Error::other("a message over the size limit"));
                }
                t.extend_from_slice(&payload);
            }
            0x8 => return Ok(Message::Close),
            0x9 => return Ok(Message::Ping(payload)),
            0xA => return Ok(Message::Pong),
            _ => return Err(std::io::Error::other("an unexpected frame")),
        }
        if fin && let Some(t) = text.take() {
            return String::from_utf8(t)
                .map(Message::Text)
                .map_err(|_| std::io::Error::other("a text message that isn't UTF-8"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_key() {
        // RFC 6455's example
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[test]
    fn frames() {
        let mut buf = Vec::new();
        write_client(&mut buf, "hello").unwrap();
        let long = "x".repeat(70_000);
        write_client(&mut buf, &long).unwrap();
        // A fragmented message: "ab" then "cd", masked
        let start = buf.len();
        frame(&mut buf, 0x1, b"ab", Some([1, 2, 3, 4])).unwrap();
        buf[start] &= 0x7f; // not fin
        frame(&mut buf, 0x0, b"cd", Some([5, 6, 7, 8])).unwrap();
        frame(&mut buf, 0x9, b"p", Some([0; 4])).unwrap();
        frame(&mut buf, 0x8, b"", Some([0; 4])).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read(&mut r, false).unwrap(), Message::Text("hello".into()));
        assert_eq!(read(&mut r, false).unwrap(), Message::Text(long));
        assert_eq!(read(&mut r, false).unwrap(), Message::Text("abcd".into()));
        assert_eq!(read(&mut r, false).unwrap(), Message::Ping(b"p".to_vec()));
        assert_eq!(read(&mut r, false).unwrap(), Message::Close);
        // Unmasked from a client is refused
        let mut buf = Vec::new();
        write(&mut buf, &Out::Text("x".into())).unwrap();
        assert!(read(&mut buf.as_slice(), false).is_err());
        assert_eq!(
            read(&mut buf.as_slice(), true).unwrap(),
            Message::Text("x".into())
        );
    }
}
