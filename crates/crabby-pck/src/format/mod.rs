//! PCK binary format constants and low-level structures.
//!
//! Reference: Godot's `core/io/file_access_pack.cpp`. Vostok-mod-loader's
//! `GDScript` parser at `vostok-mod-loader/src/pck_enumeration.gd` is the
//! working implementation cross-checked against.

pub mod directory;
pub mod header;

/// `GDPC` magic bytes at the start of every standalone PCK.
pub const MAGIC_GDPC: u32 = 0x4350_4447;

/// Pack format V2 - Godot 4.0 through 4.5. Directory follows 16 reserved dwords.
pub const PACK_FORMAT_V2: u32 = 2;

/// Pack format V3 - Godot 4.6+. Directory offset is explicit in the header.
pub const PACK_FORMAT_V3: u32 = 3;

/// Pack flag: directory is encrypted. Encrypted packs are refused.
pub const PACK_DIR_ENCRYPTED: u32 = 1;

/// Maximum reasonable length of a packed file path. Paths longer than this
/// almost certainly indicate a malformed or misaligned directory read.
pub const MAX_PATH_LEN: u32 = 4096;
