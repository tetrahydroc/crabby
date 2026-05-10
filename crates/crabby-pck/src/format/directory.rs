//! PCK directory (file-table) parsing.
//!
//! Both V2 and V3 use the same per-entry layout:
//!
//! ```text
//! path_len: u32
//! path:     [u8; path_len]     (UTF-8)
//! offset:   u64
//! size:     u64
//! md5:      [u8; 16]
//! flags:    u32
//! ```
//!
//! The directory is preceded by a `u32` count.

use std::io::{Read, Seek};

use crabby_error::{CrabbyError, Result};

use super::MAX_PATH_LEN;
use super::header::{read_exact_bytes, read_u32, read_u64};

/// A single PCK directory entry.
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Path as stored in the PCK. May or may not be `res://`-prefixed;
    /// callers normalize on use.
    pub path: String,
    /// Byte offset of the entry's data within the PCK file. For V3 with
    /// `PACK_REL_FILEBASE`, this is relative to the header's `file_base`.
    pub offset: u64,
    /// Byte length of the entry's data.
    pub size: u64,
    /// MD5 hash of the entry's data.
    pub md5: [u8; 16],
    /// Per-entry flags (reserved by Godot; surfaced raw for completeness).
    pub flags: u32,
}

/// Read the directory from the current reader position. Caller must have
/// already seeked to the directory start (e.g. via [`crate::format::header::PckHeader::read`]).
pub fn read_directory<R: Read + Seek>(reader: &mut R) -> Result<Vec<DirectoryEntry>> {
    let file_count = read_u32(reader)?;
    let mut entries = Vec::with_capacity(file_count as usize);

    for index in 0..file_count {
        let entry = read_entry(reader, index)?;
        entries.push(entry);
    }

    Ok(entries)
}

fn read_entry<R: Read + Seek>(reader: &mut R, index: u32) -> Result<DirectoryEntry> {
    let path_len = read_u32(reader)?;
    if path_len == 0 || path_len > MAX_PATH_LEN {
        return Err(CrabbyError::Pck {
            context: format!("entry {index}: suspicious path_len={path_len} (max {MAX_PATH_LEN})",),
            source: "malformed directory or misaligned read".into(),
        });
    }

    let path_bytes = read_exact_bytes(reader, path_len as usize)?;
    // Godot pads UTF-8 paths with NULs so the record aligns; trim them.
    let path = String::from_utf8(path_bytes)
        .map_err(|source| CrabbyError::Pck {
            context: format!("entry {index}: path is not valid UTF-8"),
            source: Box::new(source),
        })?
        .trim_end_matches('\0')
        .to_owned();

    let offset = read_u64(reader)?;
    let size = read_u64(reader)?;

    let md5_vec = read_exact_bytes(reader, 16)?;
    let md5: [u8; 16] = md5_vec.try_into().expect("read_exact_bytes(16) -> [u8;16]");

    let flags = read_u32(reader)?;

    Ok(DirectoryEntry {
        path,
        offset,
        size,
        md5,
        flags,
    })
}
