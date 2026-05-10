//! Outer-frame parsing: magic, version, decompression.
//!
//! A `.gdc` file begins with a 12-byte outer header:
//!
//! ```text
//! magic:              [u8; 4]   "GDSC"
//! version:            u32       100 or 101
//! decompressed_size:  u32       0 → body is raw; non-zero → body is zstd-compressed
//! ```
//!
//! The body (either raw or decompressed) then holds the metadata block,
//! identifier table, constants, line/column maps, and token stream.

use crabby_error::{CrabbyError, Result};

use crate::format::{MAGIC, OUTER_HEADER_LEN, SUPPORTED_VERSIONS, TokenizerVersion};

/// Parsed outer frame + owned body bytes.
#[derive(Debug)]
pub struct Frame {
    pub version: TokenizerVersion,
    pub body: Vec<u8>,
}

impl Frame {
    /// Parse outer header + body from raw `.gdc` bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < OUTER_HEADER_LEN {
            return Err(CrabbyError::Detokenize {
                context: format!(
                    "header truncated: got {} bytes, need {OUTER_HEADER_LEN}",
                    bytes.len(),
                ),
                source: "outer header too short".into(),
            });
        }

        if &bytes[..4] != MAGIC {
            return Err(CrabbyError::Detokenize {
                context: format!(
                    "bad magic {:02x?}; expected {:02x?} (ASCII 'GDSC')",
                    &bytes[..4],
                    MAGIC,
                ),
                source: "not a tokenized GDSC file".into(),
            });
        }

        let raw_version = read_u32_at(bytes, 4);
        let version = TokenizerVersion::from_raw(raw_version).ok_or_else(|| {
            CrabbyError::Detokenize {
                context: format!(
                    "unsupported tokenizer version {raw_version}; crabby supports {SUPPORTED_VERSIONS}",
                ),
                source: "tokenizer version out of range".into(),
            }
        })?;

        let decompressed_size = read_u32_at(bytes, 8) as usize;
        let payload = &bytes[OUTER_HEADER_LEN..];

        let body = if decompressed_size == 0 {
            payload.to_vec()
        } else {
            zstd::bulk::decompress(payload, decompressed_size).map_err(|source| {
                CrabbyError::Detokenize {
                    context: format!(
                        "zstd decompression failed (expected {decompressed_size} bytes)"
                    ),
                    source: Box::new(source),
                }
            })?
        };

        Ok(Self { version, body })
    }
}

/// Probe the tokenizer version from raw `.gdc` bytes without decompressing.
///
/// Returns `Ok(version)` for recognized payloads, `Ok(None)` for anything
/// that isn't a GDSC file at all (callers treat this as "not tokenized,
/// maybe plain text"). Genuine format errors still return `Err`.
pub fn probe_version(bytes: &[u8]) -> Result<Option<TokenizerVersion>> {
    if bytes.len() < OUTER_HEADER_LEN || &bytes[..4] != MAGIC {
        return Ok(None);
    }
    let raw = read_u32_at(bytes, 4);
    Ok(TokenizerVersion::from_raw(raw))
}

fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_recognizes_v101() {
        let bytes = [b'G', b'D', b'S', b'C', 101, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(probe_version(&bytes).unwrap(), Some(TokenizerVersion::V101));
    }

    #[test]
    fn probe_recognizes_v100() {
        let bytes = [b'G', b'D', b'S', b'C', 100, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(probe_version(&bytes).unwrap(), Some(TokenizerVersion::V100));
    }

    #[test]
    fn probe_returns_none_for_non_gdsc() {
        let bytes = b"extends Node\n";
        assert_eq!(probe_version(bytes).unwrap(), None);
    }

    #[test]
    fn probe_returns_none_for_short_input() {
        let bytes = b"GDSC";
        assert_eq!(probe_version(bytes).unwrap(), None);
    }

    #[test]
    fn parse_rejects_short_header() {
        let err = Frame::parse(b"GDSC").expect_err("short header should fail");
        match err {
            CrabbyError::Detokenize { context, .. } => {
                assert!(context.contains("header truncated"), "got: {context}");
            }
            other => panic!("expected Detokenize, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut bytes = vec![0u8; OUTER_HEADER_LEN];
        bytes[..4].copy_from_slice(b"XXXX");
        let err = Frame::parse(&bytes).expect_err("bad magic should fail");
        match err {
            CrabbyError::Detokenize { context, .. } => {
                assert!(context.contains("bad magic"), "got: {context}");
            }
            other => panic!("expected Detokenize, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_unsupported_version() {
        let mut bytes = vec![0u8; OUTER_HEADER_LEN];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        let err = Frame::parse(&bytes).expect_err("bad version should fail");
        match err {
            CrabbyError::Detokenize { context, .. } => {
                assert!(
                    context.contains("unsupported tokenizer version"),
                    "got: {context}"
                );
            }
            other => panic!("expected Detokenize, got {other:?}"),
        }
    }

    #[test]
    fn parse_accepts_raw_body() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&101u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // raw, not compressed
        bytes.extend_from_slice(b"raw body bytes");
        let frame = Frame::parse(&bytes).expect("should parse");
        assert_eq!(frame.version, TokenizerVersion::V101);
        assert_eq!(&frame.body, b"raw body bytes");
    }

    #[test]
    fn parse_decompresses_zstd_body() {
        let payload = b"hello hello hello hello hello world world world".to_vec();
        let compressed = zstd::bulk::compress(&payload, 3).expect("compress");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&101u32.to_le_bytes());
        let decompressed_size = u32::try_from(payload.len()).expect("fits u32");
        bytes.extend_from_slice(&decompressed_size.to_le_bytes());
        bytes.extend_from_slice(&compressed);

        let frame = Frame::parse(&bytes).expect("should parse");
        assert_eq!(frame.body, payload);
    }
}
