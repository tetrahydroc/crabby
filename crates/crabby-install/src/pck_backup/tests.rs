//! Unit tests for [`super`].
//!
//! Each test creates a temp dir with a synthetic "RTV.pck" (just bytes,
//! since the backup module doesn't parse PCK structure) and exercises a
//! single primitive.

use super::*;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crabby-install-pckbak-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn write_pck(&self, bytes: &[u8]) {
        fs::write(self.path.join(VANILLA_PCK_NAME), bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn hash_file_matches_known_sha256() {
    // SHA-256 of the literal string "crabby", confirmable via:
    //   echo -n "crabby" | sha256sum
    let tmp = TempDir::new("hash");
    let p = tmp.path.join("probe");
    fs::write(&p, b"crabby").unwrap();
    let h = hash_file(&p).expect("hash");
    assert_eq!(
        h,
        "115fb3a521d2adac5b442580e24d3f470d8498335539ffd6745f447845d3f24f",
    );
}

#[test]
fn classify_returns_missing_when_pck_absent() {
    let tmp = TempDir::new("missing");
    let st = classify_pck(&tmp.path, None, None).expect("classify");
    assert_eq!(st, PckState::Missing);
}

#[test]
fn classify_returns_unknown_when_no_hashes_recorded() {
    let tmp = TempDir::new("unknown-no-hashes");
    tmp.write_pck(b"some bytes");
    match classify_pck(&tmp.path, None, None).expect("classify") {
        PckState::Unknown { hash } => assert!(!hash.is_empty()),
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn classify_returns_vanilla_when_hash_matches_recorded_vanilla() {
    let tmp = TempDir::new("classify-vanilla");
    tmp.write_pck(b"vanilla pck bytes");
    let vanilla_hash = hash_file(&tmp.path.join(VANILLA_PCK_NAME)).expect("hash");
    let st = classify_pck(&tmp.path, Some(&vanilla_hash), None).expect("classify");
    assert_eq!(st, PckState::Vanilla { hash: vanilla_hash });
}

#[test]
fn classify_returns_ours_when_hash_matches_baked() {
    let tmp = TempDir::new("classify-ours");
    tmp.write_pck(b"our baked pck bytes");
    let baked_hash = hash_file(&tmp.path.join(VANILLA_PCK_NAME)).expect("hash");
    let st = classify_pck(&tmp.path, Some("not-this"), Some(&baked_hash)).expect("classify");
    assert_eq!(st, PckState::OursCurrent { hash: baked_hash });
}

#[test]
fn classify_prefers_baked_over_vanilla_when_both_match_simultaneously() {
    // Pathological: vanilla and baked happen to be identical (e.g. the
    // baked output is byte-equal to vanilla because no mods were
    // active). Either classification is technically correct;
    // OursCurrent is deliberately preferred so the install loop trusts
    // that on-disk bytes are crabby's.
    let tmp = TempDir::new("classify-tie");
    tmp.write_pck(b"degenerate identical bytes");
    let h = hash_file(&tmp.path.join(VANILLA_PCK_NAME)).expect("hash");
    let st = classify_pck(&tmp.path, Some(&h), Some(&h)).expect("classify");
    assert_eq!(st, PckState::OursCurrent { hash: h });
}

#[test]
fn ensure_backup_creates_when_missing() {
    let tmp = TempDir::new("backup-create");
    tmp.write_pck(b"vanilla payload");
    assert!(!tmp.path.join(VANILLA_PCK_BACKUP_NAME).exists());

    let h = ensure_backup(&tmp.path).expect("ensure_backup");
    let backup_bytes = fs::read(tmp.path.join(VANILLA_PCK_BACKUP_NAME)).expect("read backup");
    assert_eq!(backup_bytes, b"vanilla payload");
    assert_eq!(h, hash_file(&tmp.path.join(VANILLA_PCK_NAME)).unwrap());
}

#[test]
fn ensure_backup_is_no_op_when_backup_matches_pck() {
    // Caller pre-condition: only invoke when RTV.pck is vanilla. So in
    // steady state pck and backup will have the same bytes; no re-copy
    // should happen.
    let tmp = TempDir::new("backup-noop");
    tmp.write_pck(b"vanilla payload");
    let h = ensure_backup(&tmp.path).expect("first ensure");

    // Get the backup's mtime; the second call must not touch it.
    let bak_path = tmp.path.join(VANILLA_PCK_BACKUP_NAME);
    let mtime_before = fs::metadata(&bak_path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));

    let h2 = ensure_backup(&tmp.path).expect("second ensure");
    assert_eq!(h, h2);
    let mtime_after = fs::metadata(&bak_path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "backup must not be re-written when bytes already match",
    );
}

#[test]
fn ensure_backup_refreshes_when_pck_changed_since_last_backup() {
    // Steam-update path: vanilla RTV.pck has new bytes; the cached
    // backup is stale. ensure_backup must refresh from the current
    // (now-vanilla-but-different) bytes.
    let tmp = TempDir::new("backup-refresh");
    tmp.write_pck(b"vanilla v1");
    let h_v1 = ensure_backup(&tmp.path).expect("first ensure");

    fs::write(
        tmp.path.join(VANILLA_PCK_NAME),
        b"vanilla v2 (Steam updated)",
    )
    .unwrap();
    let h_v2 = ensure_backup(&tmp.path).expect("refresh");
    assert_ne!(h_v1, h_v2);
    let backup_bytes = fs::read(tmp.path.join(VANILLA_PCK_BACKUP_NAME)).unwrap();
    assert_eq!(backup_bytes, b"vanilla v2 (Steam updated)");
}

#[test]
fn ensure_backup_errors_when_pck_missing() {
    let tmp = TempDir::new("backup-no-src");
    let err = ensure_backup(&tmp.path).expect_err("must fail");
    let msg = format!("{err}");
    assert!(msg.contains("does not exist"), "{msg}");
}

#[test]
fn restore_from_backup_replaces_pck_with_backup_bytes() {
    let tmp = TempDir::new("restore");
    tmp.write_pck(b"vanilla payload");
    ensure_backup(&tmp.path).expect("ensure");

    fs::write(tmp.path.join(VANILLA_PCK_NAME), b"OUR BAKED OUTPUT").unwrap();
    restore_from_backup(&tmp.path).expect("restore");

    let pck_bytes = fs::read(tmp.path.join(VANILLA_PCK_NAME)).expect("read pck");
    assert_eq!(pck_bytes, b"vanilla payload");
    // Backup itself must remain.
    assert!(tmp.path.join(VANILLA_PCK_BACKUP_NAME).is_file());
}

#[test]
fn restore_errors_when_backup_missing() {
    let tmp = TempDir::new("restore-no-bak");
    tmp.write_pck(b"current pck");
    let err = restore_from_backup(&tmp.path).expect_err("must fail");
    let msg = format!("{err}");
    assert!(msg.contains("backup"), "{msg}");
}

#[test]
fn copy_atomic_creates_parent_dirs() {
    let tmp = TempDir::new("copy-parents");
    let src = tmp.path.join("src.bin");
    fs::write(&src, b"payload").unwrap();
    let dst = tmp.path.join("a/b/c/dst.bin");
    copy_atomic(&src, &dst).expect("copy");
    assert_eq!(fs::read(&dst).unwrap(), b"payload");
}
