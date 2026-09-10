//! Base64, hand-rolled.
//!
//! Serial traffic is bytes, not text: a projector answering with 0x8F is not
//! valid UTF-8 and a JSON string cannot carry it. Base64 is the standard way
//! through, and 30 lines here beats a dependency on a controller where every
//! crate is also a thing to cross-compile for i686 musl.
const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for ch in s.bytes() {
        if ch == b'=' || ch.is_ascii_whitespace() {
            continue;
        }
        let v = T.iter().position(|&t| t == ch)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_arbitrary_bytes() {
        // The case that matters: serial traffic is not text. 0x8f alone is
        // invalid UTF-8, which is the whole reason this module exists.
        for case in [vec![], vec![0x8f], vec![0, 1, 2], b"hello".to_vec(), (0u8..=255).collect()] {
            assert_eq!(decode(&encode(&case)), Some(case.clone()), "{case:?}");
        }
    }

    #[test]
    fn matches_known_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(decode("!!!!"), None);
    }
}
