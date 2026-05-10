//! PCK header parsing.
//!
//! The header layout differs between V2 and V3; this module produces a
//! unified [`PckHeader`] describing where the directory starts and how many
//! entries it holds after dispatching to the correct layout.
//!
//! Layout reference: Godot's `core/io/file_access_pack.cpp`.

use std::io::{Read, Seek, SeekFrom};

use crabby_error::{CrabbyError, Result};

use super::{MAGIC_GDPC, PACK_DIR_ENCRYPTED, PACK_FORMAT_V2, PACK_FORMAT_V3};

/// Parsed header - just the fields downstream code cares about.
#[derive(Debug, Clone, Copy)]
pub struct PckHeader {
    /// Pack format version (2 or 3).
    pub format_version: u32,
    /// Godot major version declared in the pack.
    pub godot_major: u32,
    /// Godot minor version declared in the pack.
    pub godot_minor: u32,
    /// Godot patch version declared in the pack.
    pub godot_patch: u32,
    /// Pack-level flags. Bit 0 is `PACK_DIR_ENCRYPTED`.
    pub pack_flags: u32,
    /// Base file offset. V3 with `PACK_REL_FILEBASE` makes entry offsets
    /// relative to this; V2 always absolute.
    pub file_base: u64,
    /// Absolute offset at which the file directory begins.
    pub directory_offset: u64,
}

impl PckHeader {
    /// Parse a header from `reader`, which must be positioned at the start
    /// of the PCK. On success the reader is left positioned at
    /// [`PckHeader::directory_offset`].
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let magic = read_u32(reader)?;
        if magic != MAGIC_GDPC {
            return Err(CrabbyError::Pck {
                context: format!("bad magic 0x{magic:08X}; expected 'GDPC' (standalone PCK)"),
                source: "magic mismatch".into(),
            });
        }

        let format_version = read_u32(reader)?;
        if !matches!(format_version, PACK_FORMAT_V2 | PACK_FORMAT_V3) {
            return Err(CrabbyError::Pck {
                context: format!(
                    "unsupported pack format v{format_version}; crabby supports v2/v3"
                ),
                source: "version out of range".into(),
            });
        }

        let godot_major = read_u32(reader)?;
        let godot_minor = read_u32(reader)?;
        let godot_patch = read_u32(reader)?;
        let pack_flags = read_u32(reader)?;
        let file_base = read_u64(reader)?;

        let directory_offset = if format_version == PACK_FORMAT_V3 {
            let dir_off = read_u64(reader)?;
            reader
                .seek(SeekFrom::Start(dir_off))
                .map_err(|source| CrabbyError::Pck {
                    context: format!("seeking to v3 directory at 0x{dir_off:X}"),
                    source: Box::new(source),
                })?;
            dir_off
        } else {
            // V2: skip 16 reserved dwords; directory starts immediately after.
            for _ in 0..16 {
                let _ = read_u32(reader)?;
            }
            reader
                .stream_position()
                .map_err(|source| CrabbyError::Pck {
                    context: "reading v2 directory position".into(),
                    source: Box::new(source),
                })?
        };

        if pack_flags & PACK_DIR_ENCRYPTED != 0 {
            return Err(CrabbyError::Pck {
                context: "pack directory is encrypted; crabby cannot enumerate".into(),
                source: "encrypted directories are unsupported".into(),
            });
        }

        Ok(Self {
            format_version,
            godot_major,
            godot_minor,
            godot_patch,
            pack_flags,
            file_base,
            directory_offset,
        })
    }
}

/// Read a little-endian `u32`.
pub(super) fn read_u32<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader
        .read_exact(&mut buf)
        .map_err(|source| CrabbyError::Pck {
            context: "reading u32".into(),
            source: Box::new(source),
        })?;
    Ok(u32::from_le_bytes(buf))
}

/// Read a little-endian `u64`.
pub(super) fn read_u64<R: Read>(reader: &mut R) -> Result<u64> {
    let mut buf = [0u8; 8];
    reader
        .read_exact(&mut buf)
        .map_err(|source| CrabbyError::Pck {
            context: "reading u64".into(),
            source: Box::new(source),
        })?;
    Ok(u64::from_le_bytes(buf))
}

/// Read exactly `n` bytes.
pub(super) fn read_exact_bytes<R: Read>(reader: &mut R, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    reader
        .read_exact(&mut buf)
        .map_err(|source| CrabbyError::Pck {
            context: format!("reading {n} bytes"),
            source: Box::new(source),
        })?;
    Ok(buf)
}
