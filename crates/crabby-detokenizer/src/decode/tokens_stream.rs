//! Token-stream decoder.
//!
//! Each token is a variable-length record. The first byte of the type field
//! carries a high-bit flag:
//!
//! - set (`0x80`): 8-byte record - full 4-byte type+index header followed
//!   by 4 bytes reserved for future use (currently zero).
//! - clear: 5-byte record - 4-byte type+index header, 1 byte reserved.
//!
//! Matches vostok's 5 vs 8 byte split exactly.
//! The type field layout is: low 7 bits = token type, bits 8-31 = data index.

use crabby_error::Result;

use crate::format::{TOKEN_BYTE_MASK, TOKEN_TYPE_BITS, TOKEN_TYPE_MASK};
use crate::tokens::RawToken;

use super::Cursor;

pub fn decode(cursor: &mut Cursor<'_>, count: u32) -> Result<Vec<RawToken>> {
    let mut tokens = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if cursor.remaining() == 0 {
            break; // tolerate tokenizer padding like vostok does
        }
        let first = cursor.peek_u8()?;
        let record_len = if first & TOKEN_BYTE_MASK != 0 { 8 } else { 5 };
        if cursor.remaining() < record_len {
            break;
        }

        let raw_type = cursor.read_u32()?;
        let tk_type = raw_type & TOKEN_TYPE_MASK;
        let data_index = raw_type >> TOKEN_TYPE_BITS;
        // Skip the remaining record bytes: record_len - 4 bytes already consumed by read_u32.
        let _ = cursor.take(record_len - 4)?;

        tokens.push(RawToken {
            kind: tk_type,
            data_index,
        });
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_short_record_with_type_only() {
        // 5-byte record: type byte without 0x80, then 4 bytes of packed type+index.
        // raw_type = 0x0000_0040 → type = 64 (preload), data_index = 0.
        let bytes = [0x40, 0x00, 0x00, 0x00, 0x00];
        let mut c = Cursor::new(&bytes);
        let tokens = decode(&mut c, 1).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, 64);
        assert_eq!(tokens[0].data_index, 0);
    }

    #[test]
    fn decodes_long_record_with_data_index() {
        // 8-byte record: first byte has 0x80 set → IDENTIFIER (type 2) with index 7.
        // raw_type = (7 << 8) | 2 | 0x80 = 0x00_00_07_82
        // Correction: the 0x80 flag lives in the first byte of the record, so the
        // raw_type must have its low byte's 0x80 bit set. Type=2 is plain (not 0x80),
        // but the flag bit needs to be set by the encoder to signal a long record.
        // Vostok checks `buf[offset] & 0x80`, i.e. the high bit of the FIRST byte of
        // the 4-byte type word. So raw_type = 0x00_00_07_82 → first byte 0x82 → flag
        // set → long record. type = 0x82 & 0x7f = 2 (IDENTIFIER). index = 0x07_00_00_00 >> 8 = 0x70000.
        // Rework: type must fit in 7 bits, so for a long record with type=2, data_index=7:
        //   packed = type | (index << 8) | 0x80 = 2 | (7 << 8) | 0x80 = 0x0782
        let raw = 2u32 | (7u32 << 8) | 0x80;
        let mut bytes = raw.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // 4 bytes padding to make 8-byte record
        let mut c = Cursor::new(&bytes);
        let tokens = decode(&mut c, 1).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, 2); // IDENTIFIER, 0x80 flag stripped by TOKEN_TYPE_MASK
        assert_eq!(tokens[0].data_index, 7);
    }

    #[test]
    fn stops_on_truncated_stream() {
        // count says 3 but bytes only hold 1 short record.
        let bytes = [0x40, 0, 0, 0, 0];
        let mut c = Cursor::new(&bytes);
        let tokens = decode(&mut c, 3).unwrap();
        assert_eq!(tokens.len(), 1);
    }
}
