//! Identifier-table decoder.
//!
//! Each identifier is stored as `length: u32` followed by `length` UTF-32
//! code points, each XOR'd byte-wise with `0xb6`. Code points of `0` are
//! trailing padding and get dropped.

use crabby_error::{CrabbyError, Result};

use crate::format::IDENTIFIER_XOR_MASK;

use super::Cursor;

pub fn decode(cursor: &mut Cursor<'_>, count: u32) -> Result<Vec<String>> {
    let mut idents = Vec::with_capacity(count as usize);
    for i in 0..count {
        idents.push(decode_one(cursor, i)?);
    }
    Ok(idents)
}

fn decode_one(cursor: &mut Cursor<'_>, index: u32) -> Result<String> {
    let len = cursor.read_u32()?;
    let mut out = String::with_capacity(len as usize);
    for j in 0..len {
        let bytes = cursor.take(4)?;
        let code_point = u32::from_le_bytes([
            bytes[0] ^ IDENTIFIER_XOR_MASK,
            bytes[1] ^ IDENTIFIER_XOR_MASK,
            bytes[2] ^ IDENTIFIER_XOR_MASK,
            bytes[3] ^ IDENTIFIER_XOR_MASK,
        ]);
        if code_point == 0 {
            continue;
        }
        let ch = char::from_u32(code_point).ok_or_else(|| CrabbyError::Detokenize {
            context: format!(
                "identifier {index}, code point {j}: 0x{code_point:08x} is not valid UTF-32",
            ),
            source: "invalid code point".into(),
        })?;
        out.push(ch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an identifier-table byte sequence from a list of names.
    fn encode(names: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for name in names {
            let chars: Vec<u32> = name.chars().map(|c| c as u32).collect();
            #[allow(clippy::cast_possible_truncation)] // test fixture sizes fit u32
            out.extend_from_slice(&(chars.len() as u32).to_le_bytes());
            for cp in chars {
                let raw = cp.to_le_bytes();
                out.extend_from_slice(&[
                    raw[0] ^ IDENTIFIER_XOR_MASK,
                    raw[1] ^ IDENTIFIER_XOR_MASK,
                    raw[2] ^ IDENTIFIER_XOR_MASK,
                    raw[3] ^ IDENTIFIER_XOR_MASK,
                ]);
            }
        }
        out
    }

    #[test]
    fn roundtrips_ascii_identifiers() {
        let bytes = encode(&["foo", "bar_baz", "CamelCase"]);
        let mut c = Cursor::new(&bytes);
        let idents = decode(&mut c, 3).unwrap();
        assert_eq!(idents, vec!["foo", "bar_baz", "CamelCase"]);
    }

    #[test]
    fn roundtrips_non_ascii() {
        let bytes = encode(&["héllo", "café", "日本語"]);
        let mut c = Cursor::new(&bytes);
        let idents = decode(&mut c, 3).unwrap();
        assert_eq!(idents, vec!["héllo", "café", "日本語"]);
    }

    #[test]
    fn zero_padding_is_dropped() {
        // Manually encode a name followed by zero padding.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&5u32.to_le_bytes()); // length 5, but only 3 real chars
        for cp in ['a' as u32, 'b' as u32, 'c' as u32] {
            let raw = cp.to_le_bytes();
            bytes.extend_from_slice(&[
                raw[0] ^ IDENTIFIER_XOR_MASK,
                raw[1] ^ IDENTIFIER_XOR_MASK,
                raw[2] ^ IDENTIFIER_XOR_MASK,
                raw[3] ^ IDENTIFIER_XOR_MASK,
            ]);
        }
        // Two trailing zero code points, still XOR'd.
        for _ in 0..2 {
            bytes.extend_from_slice(&[IDENTIFIER_XOR_MASK; 4]);
        }

        let mut c = Cursor::new(&bytes);
        let idents = decode(&mut c, 1).unwrap();
        assert_eq!(idents, vec!["abc"]);
    }

    #[test]
    fn empty_table_yields_empty_vec() {
        let mut c = Cursor::new(&[]);
        assert_eq!(decode(&mut c, 0).unwrap(), Vec::<String>::new());
    }
}
