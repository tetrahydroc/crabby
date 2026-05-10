//! Round-trip tests for the writer.
//!
//! Each test writes a fresh PCK via [`PckWriter`], opens it with
//! [`crate::PckArchive`], and verifies the entries read back exactly.

use std::fs;
use std::path::PathBuf;

use super::*;
use crate::PckArchive;

struct TempPath {
    path: PathBuf,
}

impl TempPath {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crabby-pck-writer-{tag}-{}-{}.pck",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        let _ = fs::remove_file(&path);
        Self { path }
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let mut tmp = self.path.clone();
        let mut name = tmp
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        name.push(".tmp");
        tmp.set_file_name(name);
        let _ = fs::remove_file(&tmp);
    }
}

#[test]
fn round_trips_a_single_v3_entry() {
    let tmp = TempPath::new("single-v3");
    let mut w = PckWriter::new_v3((4, 6, 2));
    w.add_entry("res://hello.txt", b"hi crabby".to_vec());
    w.write_to(&tmp.path).expect("write");

    let mut archive = PckArchive::open(&tmp.path).expect("open");
    assert_eq!(archive.format_version(), 3);
    assert_eq!(archive.godot_version(), (4, 6, 2));
    assert_eq!(archive.entries().len(), 1);
    assert_eq!(archive.entries()[0].path, "res://hello.txt");
    let e = archive.entries()[0].clone();
    let payload = archive.read(&e).expect("read");
    assert_eq!(payload, b"hi crabby");
}

#[test]
fn round_trips_multiple_entries_in_order() {
    let tmp = TempPath::new("multi-v3");
    let mut w = PckWriter::new_v3((4, 6, 2));
    w.add_entry("res://a.gd", b"first".to_vec());
    w.add_entry("res://Scripts/B.gd", b"second entry, longer".to_vec());
    w.add_entry("res://c.bin", vec![0u8, 1, 2, 3, 4, 5]);
    w.write_to(&tmp.path).expect("write");

    let mut archive = PckArchive::open(&tmp.path).expect("open");
    assert_eq!(archive.entries().len(), 3);
    let entries: Vec<_> = archive.entries().to_vec();
    assert_eq!(entries[0].path, "res://a.gd");
    assert_eq!(entries[1].path, "res://Scripts/B.gd");
    assert_eq!(entries[2].path, "res://c.bin");
    let payloads: Vec<_> = entries
        .iter()
        .map(|e| archive.read(e).expect("read"))
        .collect();
    assert_eq!(payloads[0], b"first");
    assert_eq!(payloads[1], b"second entry, longer");
    assert_eq!(payloads[2], vec![0u8, 1, 2, 3, 4, 5]);
}

#[test]
fn computes_md5_for_each_entry() {
    let tmp = TempPath::new("md5");
    let mut w = PckWriter::new_v3((4, 6, 2));
    let body = b"crabby v0.1.0".to_vec();
    w.add_entry("res://probe.txt", body.clone());
    w.write_to(&tmp.path).expect("write");

    let archive = PckArchive::open(&tmp.path).expect("open");
    // md5 of "crabby v0.1.0" computed independently
    use md5::Digest as _;
    let mut h = md5::Md5::new();
    h.update(&body);
    let want: [u8; 16] = h.finalize().into();
    assert_eq!(archive.entries()[0].md5, want);
}

#[test]
fn writes_v2_when_requested() {
    let tmp = TempPath::new("v2");
    let mut w = PckWriter::new_v2((3, 5, 0));
    w.add_entry("res://x.txt", b"v2 payload".to_vec());
    w.write_to(&tmp.path).expect("write");

    let mut archive = PckArchive::open(&tmp.path).expect("open");
    assert_eq!(archive.format_version(), 2);
    assert_eq!(archive.godot_version(), (3, 5, 0));
    let e = archive.entries()[0].clone();
    let payload = archive.read(&e).expect("read");
    assert_eq!(payload, b"v2 payload");
}

#[test]
fn empty_archive_round_trips() {
    let tmp = TempPath::new("empty");
    let w = PckWriter::new_v3((4, 6, 2));
    w.write_to(&tmp.path).expect("write");
    let archive = PckArchive::open(&tmp.path).expect("open");
    assert_eq!(archive.entries().len(), 0);
}

#[test]
fn write_to_creates_missing_parent_dirs() {
    let base = std::env::temp_dir().join(format!(
        "crabby-pck-nested-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let nested = base.join("a").join("b").join("c").join("out.pck");
    let mut w = PckWriter::new_v3((4, 6, 2));
    w.add_entry("res://x", b"y".to_vec());
    w.write_to(&nested).expect("write nested");
    assert!(nested.is_file());
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn write_to_overwrites_existing_target() {
    let tmp = TempPath::new("overwrite");
    fs::write(&tmp.path, b"stale prior content").expect("seed");

    let mut w = PckWriter::new_v3((4, 6, 2));
    w.add_entry("res://fresh.txt", b"fresh".to_vec());
    w.write_to(&tmp.path).expect("write");

    let mut archive = PckArchive::open(&tmp.path).expect("open");
    let e = archive.entries()[0].clone();
    let payload = archive.read(&e).expect("read");
    assert_eq!(payload, b"fresh");
}
