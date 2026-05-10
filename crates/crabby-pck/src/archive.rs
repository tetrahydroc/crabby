//! Public `PckArchive` API.
//!
//! Opens a PCK, parses its directory once, and serves entry lookups + byte
//! reads. Holds an owned [`File`](std::fs::File) handle so `read` can seek
//! without re-opening.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};
use tracing::debug;

use crate::format::directory::{DirectoryEntry, read_directory};
use crate::format::header::PckHeader;

/// A parsed PCK archive.
#[derive(Debug)]
pub struct PckArchive {
    path: PathBuf,
    reader: BufReader<File>,
    header: PckHeader,
    entries: Vec<PckEntry>,
}

/// A single file entry in the PCK.
///
/// Same shape as `format::directory::DirectoryEntry`, re-exported as the
/// crate's public surface. Callers shouldn't need to reach into `format`.
#[derive(Debug, Clone)]
pub struct PckEntry {
    /// Path as stored in the PCK (may or may not be `res://`-prefixed).
    pub path: String,
    /// Byte offset of the entry's data (resolution rules below, see
    /// [`PckArchive::read`]).
    pub offset: u64,
    /// Byte length of the entry's data.
    pub size: u64,
    /// MD5 hash of the entry's data.
    pub md5: [u8; 16],
    /// Per-entry flags.
    pub flags: u32,
}

impl From<DirectoryEntry> for PckEntry {
    fn from(raw: DirectoryEntry) -> Self {
        Self {
            path: raw.path,
            offset: raw.offset,
            size: raw.size,
            md5: raw.md5,
            flags: raw.flags,
        }
    }
}

impl PckArchive {
    /// Open and parse a PCK from `path`.
    ///
    /// Reads the header and the full directory table up front so subsequent
    /// calls to [`entries`](Self::entries) and [`read`](Self::read) are
    /// cheap. The entries themselves are lazy; their byte payloads are not
    /// loaded until [`read`](Self::read) asks for them.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).map_err(|source| CrabbyError::io_at(path.to_path_buf(), source))?;
        let mut reader = BufReader::new(file);

        let header = PckHeader::read(&mut reader)?;
        debug!(
            format = header.format_version,
            godot = format!(
                "{}.{}.{}",
                header.godot_major, header.godot_minor, header.godot_patch,
            ),
            directory_offset = header.directory_offset,
            "parsed pck header",
        );

        let entries = read_directory(&mut reader)?
            .into_iter()
            .map(PckEntry::from)
            .collect();

        Ok(Self {
            path: path.to_path_buf(),
            reader,
            header,
            entries,
        })
    }

    /// Absolute path the archive was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Pack format version (2 or 3).
    #[must_use]
    pub const fn format_version(&self) -> u32 {
        self.header.format_version
    }

    /// Godot version triple declared in the pack header.
    #[must_use]
    pub const fn godot_version(&self) -> (u32, u32, u32) {
        (
            self.header.godot_major,
            self.header.godot_minor,
            self.header.godot_patch,
        )
    }

    /// All entries in the directory, in the order the PCK declares them.
    #[must_use]
    pub fn entries(&self) -> &[PckEntry] {
        &self.entries
    }

    /// Read the bytes of a single entry.
    ///
    /// # Offset resolution
    ///
    /// Godot PCKs store entry offsets two different ways, disambiguated by
    /// the V3 `PACK_REL_FILEBASE` flag (bit 1 of `pack_flags`):
    ///
    /// - V2, or V3 without the flag: `entry.offset` is absolute from the
    ///   start of the PCK file.
    /// - V3 with the flag: `entry.offset` is relative to `header.file_base`.
    ///
    /// Road to Vostok 4.6.1 ships V3 with `pack_flags = 2`, so entries are
    /// file-base-relative. The flag is checked at read time rather than
    /// baking the resolved offset into [`PckEntry`] so the entry struct
    /// stays a faithful mirror of the on-disk directory.
    pub fn read(&mut self, entry: &PckEntry) -> Result<Vec<u8>> {
        let abs_offset = self.resolve_offset(entry.offset);

        self.reader
            .seek(SeekFrom::Start(abs_offset))
            .map_err(|source| CrabbyError::Pck {
                context: format!(
                    "seeking to entry {:?} at absolute offset 0x{abs_offset:X}",
                    entry.path,
                ),
                source: Box::new(source),
            })?;

        let size = usize::try_from(entry.size).map_err(|source| CrabbyError::Pck {
            context: format!("entry {:?}: size {} exceeds usize", entry.path, entry.size),
            source: Box::new(source),
        })?;

        let mut buf = vec![0u8; size];
        self.reader
            .read_exact(&mut buf)
            .map_err(|source| CrabbyError::Pck {
                context: format!("reading {} bytes for {:?}", size, entry.path),
                source: Box::new(source),
            })?;

        Ok(buf)
    }

    const fn resolve_offset(&self, raw_offset: u64) -> u64 {
        const PACK_REL_FILEBASE: u32 = 1 << 1;
        if self.header.format_version == crate::format::PACK_FORMAT_V3
            && (self.header.pack_flags & PACK_REL_FILEBASE) != 0
        {
            self.header.file_base + raw_offset
        } else {
            raw_offset
        }
    }

    /// Stream every entry through `transform` and write the result to
    /// `out_path` as a fresh PCK with the same Godot-version triple as
    /// the source.
    ///
    /// `transform` is invoked once per source entry with the entry's
    /// metadata and current bytes. Return `None` to copy the entry
    /// through unchanged. Return `Some((new_path, new_bytes))` to
    /// replace the entry, the new path replaces the old (extension
    /// changes are allowed; e.g. `.gdc` → `.gd`), the new bytes go in
    /// the payload, and the MD5 is recomputed.
    ///
    /// Output uses the V3 format with absolute offsets regardless of
    /// the source archive's format. Godot accepts both V2 and V3, so
    /// the upgrade is invisible to consumers.
    ///
    /// The write is atomic; bytes land at `<out_path>.tmp` first then
    /// rename over `out_path`. A crash mid-write leaves `out_path`
    /// untouched (or a stale `.tmp` next to it that the next call will
    /// overwrite).
    pub fn rewrite_to<F>(&mut self, out_path: &Path, transform: F) -> Result<()>
    where
        F: FnMut(&PckEntry, &[u8]) -> Option<(String, Vec<u8>)>,
    {
        self.rewrite_to_with_additions(out_path, transform, Vec::new())
    }

    /// Same as [`Self::rewrite_to`] but also appends `additions` as
    /// brand-new entries after streaming the source archive.
    ///
    /// Use this for files that don't exist in the vanilla PCK at all
    /// (`Lib.gd`, etc.). Each addition is `(res_path, bytes)`. Caller is
    /// responsible for ensuring `res_path` doesn't collide with any
    /// existing source path or with another addition; duplicates
    /// produce a malformed archive.
    pub fn rewrite_to_with_additions<F>(
        &mut self,
        out_path: &Path,
        mut transform: F,
        additions: Vec<(String, Vec<u8>)>,
    ) -> Result<()>
    where
        F: FnMut(&PckEntry, &[u8]) -> Option<(String, Vec<u8>)>,
    {
        let mut writer = crate::writer::PckWriter::new_v3(self.godot_version());
        let entry_count = self.entries.len();
        let entries: Vec<PckEntry> = self.entries.clone();
        for entry in &entries {
            let original_bytes = self.read(entry)?;
            match transform(entry, &original_bytes) {
                Some((new_path, new_bytes)) => writer.add_entry(new_path, new_bytes),
                None => writer.add_entry(entry.path.clone(), original_bytes),
            }
        }
        let addition_count = additions.len();
        for (path, bytes) in additions {
            writer.add_entry(path, bytes);
        }
        debug!(
            out = %out_path.display(),
            entries = entry_count,
            additions = addition_count,
            "streamed rewrite",
        );
        writer.write_to(out_path)
    }
}

#[cfg(test)]
mod tests;
