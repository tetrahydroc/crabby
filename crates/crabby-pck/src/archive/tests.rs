//! Handcrafted fixture tests for the PCK reader.
//!
//! Rather than carry binary fixtures in the repo, tests build tiny PCKs in-memory
//! and write them to a tempfile. Each helper matches exactly the on-disk
//! layout Godot writes, so tests double as format documentation.

use std::fs;
use std::path::PathBuf;

use super::*;

/// Self-cleaning tempfile, process-id + test-tag scoped.
struct TempPck {
    path: PathBuf,
}

impl TempPck {
    fn new(tag: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crabby-pck-{tag}-{}-{}.pck",
            std::process::id(),
            // nanos since unix epoch gives cross-test uniqueness even when
            // two tests in this file happen to share a tag suffix by mistake.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        fs::write(&path, bytes).expect("write fixture pck");
        Self { path }
    }
}

impl Drop for TempPck {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Fluent PCK byte-builder. Call chain mirrors the on-disk field order so
/// each test's intent is obvious.
struct PckBuilder {
    bytes: Vec<u8>,
}

impl PckBuilder {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }
    fn u32(mut self, v: u32) -> Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(mut self, b: &[u8]) -> Self {
        self.bytes.extend_from_slice(b);
        self
    }
    /// Pad with zero bytes to reach absolute offset `target`.
    fn pad_to(mut self, target: usize) -> Self {
        assert!(
            self.bytes.len() <= target,
            "pad_to({target}) but already at {}",
            self.bytes.len(),
        );
        self.bytes.resize(target, 0);
        self
    }
    fn build(self) -> Vec<u8> {
        self.bytes
    }
    fn len(&self) -> usize {
        self.bytes.len()
    }
}

fn write_entry(b: PckBuilder, path: &str, offset: u64, size: u64) -> PckBuilder {
    let path_bytes = path.as_bytes();
    let path_len = u32::try_from(path_bytes.len()).expect("test path fits u32");
    b.u32(path_len)
        .bytes(path_bytes)
        .u64(offset)
        .u64(size)
        .bytes(&[0u8; 16]) // md5 - not verified in these tests
        .u32(0) // per-entry flags
}

// ------------------------------------------------------------------ V2

/// Build a valid minimal V2 PCK with two entries carrying real data.
fn build_v2_pck() -> Vec<u8> {
    // Header: magic + format(2) + gd(3,5,0) + pack_flags(0) + file_base(0)
    // + 16 reserved dwords, then directory.
    let header = PckBuilder::new()
        .u32(0x4350_4447) // "GDPC"
        .u32(2)
        .u32(3)
        .u32(5)
        .u32(0)
        .u32(0) // flags
        .u64(0); // file_base
    let header = (0..16).fold(header, |b, _| b.u32(0));

    // Directory: count, then entries. We won't know payload offsets until
    // we've measured the directory's size, so build directory first into a
    // scratch builder, measure, then fix up offsets.
    let data_a = b"hello crabby\n";
    let data_b = b"second file";

    // First pass: placeholder offsets of 0 so we can size the directory.
    let dir_scratch = {
        let b = PckBuilder::new().u32(2);
        let b = write_entry(b, "res://a.txt", 0, data_a.len() as u64);
        write_entry(b, "res://sub/b.bin", 0, data_b.len() as u64)
    };

    let data_start = header.len() + dir_scratch.len();
    let offset_a = data_start as u64;
    let offset_b = (data_start + data_a.len()) as u64;

    // Second pass: real offsets.
    let dir = {
        let b = PckBuilder::new().u32(2);
        let b = write_entry(b, "res://a.txt", offset_a, data_a.len() as u64);
        write_entry(b, "res://sub/b.bin", offset_b, data_b.len() as u64)
    };

    header
        .bytes(&dir.build())
        .bytes(data_a)
        .bytes(data_b)
        .build()
}

#[test]
fn opens_v2_pck_and_reads_entries() {
    let pck = TempPck::new("v2-basic", &build_v2_pck());
    let mut archive = PckArchive::open(&pck.path).expect("open v2");

    assert_eq!(archive.format_version(), 2);
    assert_eq!(archive.godot_version(), (3, 5, 0));
    assert_eq!(archive.entries().len(), 2);
    assert_eq!(archive.entries()[0].path, "res://a.txt");
    assert_eq!(archive.entries()[1].path, "res://sub/b.bin");

    // Clone to drop the borrow before calling &mut self read().
    let e0 = archive.entries()[0].clone();
    let e1 = archive.entries()[1].clone();

    let payload_a = archive.read(&e0).expect("read a");
    let payload_b = archive.read(&e1).expect("read b");
    assert_eq!(payload_a, b"hello crabby\n");
    assert_eq!(payload_b, b"second file");
}

// ------------------------------------------------------------------ V3

/// Build a valid minimal V3 PCK. The directory lives at the end of the file
/// and the header carries its absolute offset.
///
/// Uses `pack_flags = 0` (absolute offsets); the rel-filebase variant is
/// exercised separately below.
fn build_v3_pck_absolute() -> Vec<u8> {
    let data_a = b"v3 first\n";
    let data_b = b"v3 second entry";

    // Header: magic + format(3) + gd(4,6,1) + pack_flags + file_base
    // + directory_offset (u64 placeholder), then data, then directory.
    let partial_header_len = 4 + 4 + 12 + 4 + 8 + 8; // 40 bytes total before payloads.
    let data_start = partial_header_len;
    let offset_a = data_start as u64;
    let offset_b = (data_start + data_a.len()) as u64;
    let directory_offset = (data_start + data_a.len() + data_b.len()) as u64;

    let header = PckBuilder::new()
        .u32(0x4350_4447)
        .u32(3)
        .u32(4)
        .u32(6)
        .u32(1)
        .u32(0) // pack_flags
        .u64(0) // file_base
        .u64(directory_offset);

    let dir = {
        let b = PckBuilder::new().u32(2);
        let b = write_entry(b, "res://alpha.gd", offset_a, data_a.len() as u64);
        write_entry(b, "res://Scripts/Beta.gd", offset_b, data_b.len() as u64)
    };

    header
        .bytes(data_a)
        .bytes(data_b)
        .bytes(&dir.build())
        .build()
}

#[test]
fn opens_v3_pck_and_reads_entries() {
    let pck = TempPck::new("v3-abs", &build_v3_pck_absolute());
    let mut archive = PckArchive::open(&pck.path).expect("open v3");

    assert_eq!(archive.format_version(), 3);
    assert_eq!(archive.godot_version(), (4, 6, 1));
    assert_eq!(archive.entries().len(), 2);

    let entries: Vec<_> = archive.entries().to_vec();
    let payloads: Vec<_> = entries
        .iter()
        .map(|e| archive.read(e).expect("read"))
        .collect();
    assert_eq!(payloads[0], b"v3 first\n");
    assert_eq!(payloads[1], b"v3 second entry");
}

// ------------------------------------------------------------------ V3 rel-filebase

/// V3 with `PACK_REL_FILEBASE` (`pack_flags` bit 1). Entry offsets are measured
/// from `file_base`, not from the file start. Road to Vostok 4.6.1 uses this.
fn build_v3_pck_rel_filebase() -> Vec<u8> {
    let data_a = b"rel-base entry";
    let file_base: u64 = 0x40; // arbitrary; must be <= data_start
    let partial_header_len = 4 + 4 + 12 + 4 + 8 + 8; // 40
    // Pad so data_start lives at an offset >= file_base, and rel offsets are
    // meaningful (i.e. abs = file_base + rel).
    let data_start: usize = 0x60;
    let rel_offset_a = data_start as u64 - file_base;

    let mut bytes = PckBuilder::new()
        .u32(0x4350_4447)
        .u32(3)
        .u32(4)
        .u32(6)
        .u32(1)
        .u32(0b10) // PACK_REL_FILEBASE
        .u64(file_base)
        .u64(0); // placeholder directory_offset, overwrite below.

    // Pad from end-of-header (40) to data_start (0x60).
    bytes = bytes.pad_to(data_start);
    let after_data = data_start + data_a.len();
    let directory_offset = after_data as u64;

    // Rewrite the placeholder directory_offset at bytes[32..40].
    let mut bytes = bytes.bytes(data_a).build();
    bytes[32..40].copy_from_slice(&directory_offset.to_le_bytes());

    // Append directory.
    let dir = {
        let b = PckBuilder::new().u32(1);
        write_entry(b, "res://rel.gd", rel_offset_a, data_a.len() as u64)
    };
    bytes.extend_from_slice(&dir.build());

    assert_eq!(partial_header_len, 40); // self-doc for the 32..40 slice.
    bytes
}

#[test]
fn v3_rel_filebase_resolves_offsets_correctly() {
    let pck = TempPck::new("v3-rel", &build_v3_pck_rel_filebase());
    let mut archive = PckArchive::open(&pck.path).expect("open v3 rel");

    let entries: Vec<_> = archive.entries().to_vec();
    assert_eq!(entries.len(), 1);
    let payload = archive.read(&entries[0]).expect("read");
    assert_eq!(payload, b"rel-base entry");
}

// ------------------------------------------------------------------ Errors

#[test]
fn rejects_non_pck_magic() {
    let pck = TempPck::new("bad-magic", b"XXXX\x02\0\0\0");
    let err = PckArchive::open(&pck.path).expect_err("should reject");
    match err {
        CrabbyError::Pck { context, .. } => {
            assert!(context.contains("bad magic"), "got: {context}");
        }
        other => panic!("expected Pck, got {other:?}"),
    }
}

#[test]
fn rejects_unsupported_version() {
    let bytes = PckBuilder::new()
        .u32(0x4350_4447)
        .u32(99) // future format
        .u32(4)
        .u32(99)
        .u32(0)
        .u32(0)
        .u64(0)
        .build();
    let pck = TempPck::new("bad-version", &bytes);
    let err = PckArchive::open(&pck.path).expect_err("should reject");
    match err {
        CrabbyError::Pck { context, .. } => {
            assert!(
                context.contains("unsupported pack format"),
                "got: {context}"
            );
        }
        other => panic!("expected Pck, got {other:?}"),
    }
}

#[test]
fn rejects_encrypted_directory() {
    // Minimal V2 with PACK_DIR_ENCRYPTED flag set.
    let header = PckBuilder::new()
        .u32(0x4350_4447)
        .u32(2)
        .u32(3)
        .u32(5)
        .u32(0)
        .u32(0b1) // PACK_DIR_ENCRYPTED
        .u64(0);
    let header = (0..16).fold(header, |b, _| b.u32(0));
    let bytes = header.u32(0).build(); // empty directory, header-level rejection kicks first

    let pck = TempPck::new("encrypted", &bytes);
    let err = PckArchive::open(&pck.path).expect_err("should reject");
    match err {
        CrabbyError::Pck { context, .. } => {
            assert!(context.contains("encrypted"), "got: {context}");
        }
        other => panic!("expected Pck, got {other:?}"),
    }
}

#[test]
fn rejects_suspicious_path_length() {
    // V2 header, then a directory with count=1 and path_len=999_999.
    let header = PckBuilder::new()
        .u32(0x4350_4447)
        .u32(2)
        .u32(3)
        .u32(5)
        .u32(0)
        .u32(0)
        .u64(0);
    let header = (0..16).fold(header, |b, _| b.u32(0));
    let bytes = header.u32(1).u32(999_999).build();

    let pck = TempPck::new("bad-path-len", &bytes);
    let err = PckArchive::open(&pck.path).expect_err("should reject");
    match err {
        CrabbyError::Pck { context, .. } => {
            assert!(context.contains("path_len"), "got: {context}");
        }
        other => panic!("expected Pck, got {other:?}"),
    }
}

// ------------------------------------------------------------------ rewrite_to

#[test]
fn rewrite_to_passes_entries_through_unchanged_by_default() {
    let pck = TempPck::new("rewrite-passthrough", &build_v3_pck_absolute());
    let mut archive = PckArchive::open(&pck.path).expect("open");

    let out_path = std::env::temp_dir().join(format!(
        "crabby-pck-rewrite-out-{}-{}.pck",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&out_path);

    archive
        .rewrite_to(&out_path, |_e, _bytes| None)
        .expect("rewrite");

    let mut out = PckArchive::open(&out_path).expect("open out");
    assert_eq!(out.entries().len(), 2);
    let e0 = out.entries()[0].clone();
    let e1 = out.entries()[1].clone();
    assert_eq!(out.read(&e0).expect("read"), b"v3 first\n");
    assert_eq!(out.read(&e1).expect("read"), b"v3 second entry");
    assert_eq!(out.entries()[0].path, "res://alpha.gd");
    assert_eq!(out.entries()[1].path, "res://Scripts/Beta.gd");

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn rewrite_to_replaces_entry_bytes_and_recomputes_md5() {
    let pck = TempPck::new("rewrite-bytes", &build_v3_pck_absolute());
    let mut archive = PckArchive::open(&pck.path).expect("open");

    let out_path = std::env::temp_dir().join(format!(
        "crabby-pck-rewrite-bytes-out-{}-{}.pck",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&out_path);

    archive
        .rewrite_to(&out_path, |entry, bytes| {
            if entry.path == "res://alpha.gd" {
                Some((entry.path.clone(), b"REPLACED CONTENT, MUCH LARGER THAN ORIGINAL".to_vec()))
            } else {
                let _ = bytes; // observe-only path
                None
            }
        })
        .expect("rewrite");

    let mut out = PckArchive::open(&out_path).expect("open out");
    let e0 = out.entries()[0].clone();
    let e1 = out.entries()[1].clone();
    assert_eq!(out.read(&e0).expect("read"), b"REPLACED CONTENT, MUCH LARGER THAN ORIGINAL");
    // MD5 was recomputed for the replaced entry.
    use md5::Digest as _;
    let mut h = md5::Md5::new();
    h.update(b"REPLACED CONTENT, MUCH LARGER THAN ORIGINAL");
    let want: [u8; 16] = h.finalize().into();
    assert_eq!(e0.md5, want);
    // Untouched entry kept.
    assert_eq!(out.read(&e1).expect("read"), b"v3 second entry");

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn rewrite_to_changes_entry_path() {
    let pck = TempPck::new("rewrite-path", &build_v3_pck_absolute());
    let mut archive = PckArchive::open(&pck.path).expect("open");

    let out_path = std::env::temp_dir().join(format!(
        "crabby-pck-rewrite-path-out-{}-{}.pck",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&out_path);

    archive
        .rewrite_to(&out_path, |entry, bytes| {
            if entry.path == "res://Scripts/Beta.gd" {
                // Simulate the .gdc → .gd rename the bake will do.
                Some(("res://Scripts/Beta.txt".to_owned(), bytes.to_vec()))
            } else {
                None
            }
        })
        .expect("rewrite");

    let out = PckArchive::open(&out_path).expect("open out");
    let paths: Vec<_> = out.entries().iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths,
        vec!["res://alpha.gd".to_owned(), "res://Scripts/Beta.txt".to_owned()],
    );

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn rewrite_to_handles_size_shrink() {
    let pck = TempPck::new("rewrite-shrink", &build_v3_pck_absolute());
    let mut archive = PckArchive::open(&pck.path).expect("open");

    let out_path = std::env::temp_dir().join(format!(
        "crabby-pck-rewrite-shrink-out-{}-{}.pck",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&out_path);

    archive
        .rewrite_to(&out_path, |entry, _bytes| {
            if entry.path == "res://Scripts/Beta.gd" {
                // Original was 15 bytes; shrink to 1.
                Some((entry.path.clone(), b"x".to_vec()))
            } else {
                None
            }
        })
        .expect("rewrite");

    let mut out = PckArchive::open(&out_path).expect("open out");
    let e0 = out.entries()[0].clone();
    let e1 = out.entries()[1].clone();
    // First entry still readable at its (recomputed) offset.
    assert_eq!(out.read(&e0).expect("read"), b"v3 first\n");
    assert_eq!(out.read(&e1).expect("read"), b"x");

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn open_missing_file_yields_io_error() {
    let path = std::env::temp_dir().join("crabby-pck-missing-nowhere.pck");
    let err = PckArchive::open(&path).expect_err("should fail");
    match err {
        CrabbyError::Io { path: p, .. } => {
            assert_eq!(p.as_deref(), Some(path.as_path()));
        }
        other => panic!("expected Io, got {other:?}"),
    }
}
