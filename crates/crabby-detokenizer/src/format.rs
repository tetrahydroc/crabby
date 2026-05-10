//! Binary-format constants and the tokenizer version type.
//!
//! These mirror the constants in Godot's tokenizer source. Vostok's
//! `gdsc_detokenizer.gd` has the same values in `GDScript` form.

/// `GDSC` as a little-endian `u32` - the outer-header magic.
pub const MAGIC: &[u8; 4] = b"GDSC";

/// Outer-header size: `magic(4)` + `version(4)` + `decompressed_size(4)`.
pub const OUTER_HEADER_LEN: usize = 12;

/// Supported tokenizer versions.
///
/// v100 - Godot 4.0-4.4. v101 - Godot 4.5-4.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerVersion {
    /// Godot 4.0-4.4.
    V100,
    /// Godot 4.5-4.6.
    V101,
}

impl TokenizerVersion {
    /// Decode a raw `u32` from the outer header.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            100 => Some(Self::V100),
            101 => Some(Self::V101),
            _ => None,
        }
    }

    /// Raw `u32` representation as written in the header.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::V100 => 100,
            Self::V101 => 101,
        }
    }

    /// Byte size of the metadata block following the (possibly decompressed)
    /// body header. v100 adds 4 bytes of padding before `token_count`.
    #[must_use]
    pub const fn meta_block_size(self) -> usize {
        match self {
            Self::V100 => 20,
            Self::V101 => 16,
        }
    }

    /// Offset within the metadata block where `token_count` sits.
    #[must_use]
    pub const fn token_count_offset(self) -> usize {
        match self {
            Self::V100 => 16,
            Self::V101 => 12,
        }
    }
}

/// Human-readable list of supported versions; used in error messages.
pub const SUPPORTED_VERSIONS: &str = "v100 (Godot 4.0-4.4), v101 (Godot 4.5-4.6)";

// --- token stream encoding ------------------------------------------------

/// Number of bits the token type occupies in the packed-type field.
pub const TOKEN_TYPE_BITS: u32 = 8;

/// Mask for the token type (low 7 bits of the first byte).
pub const TOKEN_TYPE_MASK: u32 = (1 << (TOKEN_TYPE_BITS - 1)) - 1;

/// High bit of the first type byte: set → 8-byte token record (with data
/// index), clear → 5-byte record (type only, data index in upper bits).
pub const TOKEN_BYTE_MASK: u8 = 0x80;

/// XOR mask applied to each UTF-32 code point in the identifier table.
pub const IDENTIFIER_XOR_MASK: u8 = 0xb6;
