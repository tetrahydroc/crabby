//! Writer for Godot PCK archives.
//!
//! Mirrors the binary format documented in [`crate::format`]; produces V2 or
//! V3 archives that Godot's `core/io/file_access_pack.cpp` accepts.
//!
//! Layout strategy: header → directory → payload. For V3 we always write
//! absolute entry offsets (the `PACK_REL_FILEBASE` flag is honored by
//! Godot but adds nothing for our use case). Entries are emitted in
//! insertion order; lookups are by path so order is not load-bearing for
//! correctness, but downstream tooling expects a stable layout so we
//! preserve it.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use crabby_pck::PckWriter;
//!
//! let mut w = PckWriter::new_v3((4, 6, 2));
//! w.add_entry("res://hello.txt", b"hi crabby".to_vec());
//! w.write_to(Path::new("/tmp/out.pck"))?;
//! # Ok::<(), crabby_error::CrabbyError>(())
//! ```

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};
use md5::{Digest, Md5};
use tracing::debug;

use crate::format::{MAGIC_GDPC, PACK_FORMAT_V2, PACK_FORMAT_V3};

/// One entry queued for emission.
struct Entry {
    path: String,
    bytes: Vec<u8>,
    md5: [u8; 16],
}

/// Builder for a Godot PCK archive.
pub struct PckWriter {
    format_version: u32,
    godot_version: (u32, u32, u32),
    entries: Vec<Entry>,
}

impl PckWriter {
    /// Start a fresh V3 archive declaring the supplied Godot version triple.
    /// V3 is what Godot 4.6+ writes; prefer this for new archives.
    #[must_use]
    pub fn new_v3(godot_version: (u32, u32, u32)) -> Self {
        Self {
            format_version: PACK_FORMAT_V3,
            godot_version,
            entries: Vec::new(),
        }
    }

    /// Start a fresh V2 archive declaring the supplied Godot version triple.
    /// V2 is the Godot 4.0-4.5 format; only use this when a downstream
    /// consumer specifically requires it.
    #[must_use]
    pub fn new_v2(godot_version: (u32, u32, u32)) -> Self {
        Self {
            format_version: PACK_FORMAT_V2,
            godot_version,
            entries: Vec::new(),
        }
    }

    /// Append `bytes` at `path`. Computes the entry's MD5 immediately.
    /// Inserting two entries with the same path is permitted at the
    /// builder level; it's the caller's responsibility not to do so.
    /// Duplicates produce a malformed archive that Godot may reject or
    /// load only one of.
    pub fn add_entry(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        let mut hasher = Md5::new();
        hasher.update(&bytes);
        let md5: [u8; 16] = hasher.finalize().into();
        self.entries.push(Entry {
            path: path.into(),
            bytes,
            md5,
        });
    }

    /// Number of entries queued.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Write the archive to `out_path` atomically (`<out_path>.tmp` + rename).
    /// Creates parent directories as needed. Existing files at `out_path`
    /// are removed first since `rename` won't overwrite on Windows.
    pub fn write_to(self, out_path: &Path) -> Result<()> {
        if let Some(parent) = out_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|source| CrabbyError::io_at(parent.to_path_buf(), source))?;
        }

        let tmp_path = temp_path_for(out_path);
        let entry_count = self.entries.len();
        self.write_direct(&tmp_path)?;

        if out_path.exists() {
            fs::remove_file(out_path)
                .map_err(|source| CrabbyError::io_at(out_path.to_path_buf(), source))?;
        }
        fs::rename(&tmp_path, out_path).map_err(|source| CrabbyError::Pck {
            context: format!(
                "renaming {} → {}",
                tmp_path.display(),
                out_path.display(),
            ),
            source: Box::new(source),
        })?;

        debug!(
            path = %out_path.display(),
            entries = entry_count,
            "wrote pck",
        );
        Ok(())
    }

    /// Non-atomic write directly to `path`. Test/internal use; production
    /// callers should prefer [`write_to`] which gives crash-safety.
    ///
    /// Layout (mirrors what Godot's `core/io/file_access_pack.cpp` emits
    /// for standalone main packs):
    ///
    /// ```text
    /// header (V3=40B, V2=96B)
    /// [zero-pad to file_base for V3, none for V2]
    /// payload (every entry's bytes, contiguous)
    /// directory (at file end; header.dir_off points here)
    /// ```
    ///
    /// V3 standalone packs use `PACK_REL_FILEBASE` and put `file_base` at
    /// the start of the payload. RTV's vanilla pack uses `file_base = 0x70`
    /// (header end is 0x28; the gap is reserved). Godot's main-pack
    /// loader expects this shape; the directory-first layout the reader
    /// also accepts is for embedded packs only.
    fn write_direct(self, path: &Path) -> Result<()> {
        let file = File::create(path)
            .map_err(|source| CrabbyError::io_at(path.to_path_buf(), source))?;
        let mut w = BufWriter::new(file);

        let header_len = match self.format_version {
            PACK_FORMAT_V3 => 40usize,
            PACK_FORMAT_V2 => 96usize,
            other => {
                return Err(CrabbyError::Pck {
                    context: format!("write_direct: unsupported format version {other}"),
                    source: "writer accepts v2/v3 only".into(),
                });
            }
        };

        // V3 standalone main-pack layout (mirrors what Godot writes and
        // what its main-pack loader expects):
        //   header (40B) → pad to file_base=0x70 → payload → directory at end
        //   PACK_REL_FILEBASE flag set; entry offsets relative to file_base.
        //
        // V2 layout (older format, embedded-pack-style):
        //   header (96B) → directory immediately → payload at end
        //   no PACK_REL_FILEBASE; entry offsets absolute.
        //
        // Crabby targets V3 for all real workloads; V2 support is for
        // round-tripping older archives only.
        let v3 = self.format_version == PACK_FORMAT_V3;
        let (file_base, pack_flags) = if v3 {
            (0x70u64, 1u32 << 1)
        } else {
            (0u64, 0u32)
        };

        let total_payload: u64 = self.entries.iter().map(|e| e.bytes.len() as u64).sum();

        // V3: payload first (after pad), directory at end.
        // V2: directory first (right after header), payload after directory.
        let v2_dir_size: u64 = if v3 {
            0
        } else {
            (4 + self
                .entries
                .iter()
                .map(|e| 4 + e.path.len() + 8 + 8 + 16 + 4)
                .sum::<usize>()) as u64
        };
        let payload_start: u64 = if v3 {
            file_base
        } else {
            header_len as u64 + v2_dir_size
        };
        let directory_offset: u64 = if v3 {
            payload_start + total_payload
        } else {
            header_len as u64
        };

        // --- Header
        write_u32(&mut w, MAGIC_GDPC)?;
        write_u32(&mut w, self.format_version)?;
        write_u32(&mut w, self.godot_version.0)?;
        write_u32(&mut w, self.godot_version.1)?;
        write_u32(&mut w, self.godot_version.2)?;
        write_u32(&mut w, pack_flags)?;
        write_u64(&mut w, file_base)?;
        if self.format_version == PACK_FORMAT_V3 {
            write_u64(&mut w, directory_offset)?;
        } else {
            for _ in 0..16 {
                write_u32(&mut w, 0)?; // V2 reserved dwords
            }
        }

        // Pre-compute every entry's absolute payload offset so the
        // directory can reference them, regardless of write order.
        let mut entry_offsets: Vec<u64> = Vec::with_capacity(self.entries.len());
        {
            let mut cursor = payload_start;
            for entry in &self.entries {
                entry_offsets.push(cursor);
                cursor += entry.bytes.len() as u64;
            }
        }

        let count_u32 = u32::try_from(self.entries.len()).map_err(|source| CrabbyError::Pck {
            context: format!("entry count {} exceeds u32", self.entries.len()),
            source: Box::new(source),
        })?;

        if v3 {
            // V3: header → pad → payload → directory.
            let mut written: u64 = header_len as u64;
            while written < payload_start {
                write_u32(&mut w, 0)?;
                written += 4;
            }
            debug_assert_eq!(written, payload_start);
            for entry in &self.entries {
                write_all(&mut w, &entry.bytes)?;
            }
            self.write_directory(&mut w, count_u32, &entry_offsets, file_base, v3)?;
        } else {
            // V2: header → directory → payload.
            self.write_directory(&mut w, count_u32, &entry_offsets, file_base, v3)?;
            for entry in &self.entries {
                write_all(&mut w, &entry.bytes)?;
            }
        }

        let _ = directory_offset; // referenced only via header (above), keep variable for clarity.

        w.flush().map_err(|source| CrabbyError::Pck {
            context: format!("flushing pck writer at {}", path.display()),
            source: Box::new(source),
        })?;
        Ok(())
    }

    fn write_directory<W: Write>(
        &self,
        w: &mut W,
        count: u32,
        entry_offsets: &[u64],
        file_base: u64,
        v3: bool,
    ) -> Result<()> {
        write_u32(w, count)?;
        for (entry, abs_offset) in self.entries.iter().zip(entry_offsets.iter()) {
            let path_bytes = entry.path.as_bytes();
            let path_len = u32::try_from(path_bytes.len()).map_err(|source| CrabbyError::Pck {
                context: format!("path len {} exceeds u32 ({:?})", path_bytes.len(), entry.path),
                source: Box::new(source),
            })?;
            write_u32(w, path_len)?;
            write_all(w, path_bytes)?;
            let stored_offset = if v3 { abs_offset - file_base } else { *abs_offset };
            write_u64(w, stored_offset)?;
            write_u64(w, entry.bytes.len() as u64)?;
            write_all(w, &entry.md5)?;
            write_u32(w, 0)?; // per-entry flags - RTV doesn't use any
        }
        Ok(())
    }
}

fn temp_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    target.with_file_name(name)
}

fn write_u32<W: Write>(w: &mut W, v: u32) -> Result<()> {
    w.write_all(&v.to_le_bytes()).map_err(|source| CrabbyError::Pck {
        context: format!("writing u32={v}"),
        source: Box::new(source),
    })
}

fn write_u64<W: Write>(w: &mut W, v: u64) -> Result<()> {
    w.write_all(&v.to_le_bytes()).map_err(|source| CrabbyError::Pck {
        context: format!("writing u64={v}"),
        source: Box::new(source),
    })
}

fn write_all<W: Write>(w: &mut W, bytes: &[u8]) -> Result<()> {
    w.write_all(bytes).map_err(|source| CrabbyError::Pck {
        context: format!("writing {} bytes", bytes.len()),
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests;
