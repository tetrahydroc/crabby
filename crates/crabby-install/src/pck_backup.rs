//! Vanilla-PCK backup management for the PCK-rewrite install path.
//!
//! The PCK-rewrite design (see `docs/PCK_REWRITE_PLAN.md`) treats vanilla
//! `RTV.pck` as the source of truth and the modified PCK as ephemeral.
//! This module owns the backup file (`RTV.pck.vanilla.bak`), the hashes
//! used to detect drift, and the restore primitive.
//!
//! All operations are idempotent and crash-safe: the backup is created
//! atomically (`.tmp` + rename), and a partial restore leaves the
//! original backup in place for the next attempt.
//!
//! # Hash semantics
//!
//! Hashes are SHA-256 of the entire PCK file, used to classify
//! the current `RTV.pck` against the manifest's recorded hashes:
//!
//! - **Vanilla**: matches `manifest.vanilla_pck_hash`. Steam may have
//!   re-verified or the user may have just installed; either way the
//!   backup needs refreshing before baking.
//! - **OursCurrent**: matches `manifest.last_baked_pck_hash`. The prior
//!   bake is in place; either reuse or re-bake from the backup.
//! - **Unknown**: matches neither. Could be a foreign loader's output,
//!   a corrupted file, or a vanilla never seen before. Restore from
//!   backup before doing anything else.
//!
//! See [`PckState`] for the classification function.

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::artifacts::{VANILLA_PCK_BACKUP_NAME, VANILLA_PCK_NAME};

/// SHA-256 hash, hex-encoded (lowercase, no separators).
pub type PckHash = String;

/// Classification of the current `RTV.pck` against known reference hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PckState {
    /// Current `RTV.pck` matches `vanilla_hash`, i.e. Steam-verified or
    /// freshly installed. Backup needs refreshing if its hash differs.
    Vanilla {
        /// Hash of the current `RTV.pck`.
        hash: PckHash,
    },
    /// Current `RTV.pck` is the result of our last bake.
    OursCurrent {
        /// Hash of the current `RTV.pck` (= `last_baked_hash`).
        hash: PckHash,
    },
    /// Current `RTV.pck` matches neither known hash. Restore from backup
    /// (if available) before baking.
    Unknown {
        /// Hash of the current `RTV.pck`.
        hash: PckHash,
    },
    /// `RTV.pck` doesn't exist at all. Backup is the only source.
    Missing,
}

/// Compute the SHA-256 of a file, streaming to avoid allocating a
/// multi-gigabyte buffer for the live PCK.
pub fn hash_file(path: &Path) -> Result<PckHash> {
    let file = File::open(path).map_err(|source| CrabbyError::io_at(path.to_path_buf(), source))?;
    let mut reader = BufReader::with_capacity(1 << 20, file); // 1 MiB chunks
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|source| CrabbyError::io_at(path.to_path_buf(), source))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(nibble(b >> 4));
        out.push(nibble(b & 0xf));
    }
    out
}

const fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + (n - 10)) as char,
        _ => '?',
    }
}

/// Absolute path to `RTV.pck` inside `game_dir`.
#[must_use]
pub fn pck_path(game_dir: &Path) -> PathBuf {
    game_dir.join(VANILLA_PCK_NAME)
}

/// Absolute path to the vanilla backup inside `game_dir`.
#[must_use]
pub fn backup_path(game_dir: &Path) -> PathBuf {
    game_dir.join(VANILLA_PCK_BACKUP_NAME)
}

/// Classify the current `RTV.pck` against known hashes.
///
/// `vanilla_hash` and `baked_hash` are the manifest's recorded
/// references, with `None` for either when no manifest exists or no
/// prior bake has happened.
pub fn classify_pck(
    game_dir: &Path,
    vanilla_hash: Option<&str>,
    baked_hash: Option<&str>,
) -> Result<PckState> {
    let pck = pck_path(game_dir);
    if !pck.is_file() {
        return Ok(PckState::Missing);
    }
    let hash = hash_file(&pck)?;
    if baked_hash.is_some_and(|h| h == hash) {
        return Ok(PckState::OursCurrent { hash });
    }
    if vanilla_hash.is_some_and(|h| h == hash) {
        return Ok(PckState::Vanilla { hash });
    }
    Ok(PckState::Unknown { hash })
}

/// Ensure a vanilla backup exists at [`backup_path`] containing the
/// current `RTV.pck`'s bytes.
///
/// **Caller contract**: only invoke when `RTV.pck` is known to be
/// vanilla (use [`classify_pck`] upstream to gate). Calling this with
/// our baked output as the source would back up the wrong thing.
///
/// Behavior:
///
/// 1. `RTV.pck` missing: error.
/// 2. Backup missing: copy and return new hash.
/// 3. Backup exists and matches current `RTV.pck`'s hash: no-op,
///    return existing hash.
/// 4. Backup exists but differs from `RTV.pck`'s hash: overwrite
///    (Steam updated the game; the new vanilla bytes are now truth).
///    Log a warning so this doesn't slip past silently.
///
/// Returns the backup's hash on success.
pub fn ensure_backup(game_dir: &Path) -> Result<PckHash> {
    let bak = backup_path(game_dir);
    let pck = pck_path(game_dir);

    if !pck.is_file() {
        return Err(CrabbyError::Bake {
            context: format!("ensure_backup: {} does not exist", pck.display()),
            source: "no source pck to back up from".into(),
        });
    }

    let pck_hash = hash_file(&pck)?;

    if bak.is_file() {
        let bak_hash = hash_file(&bak)?;
        if bak_hash == pck_hash {
            debug!(hash = %bak_hash, "backup already matches current RTV.pck");
            return Ok(bak_hash);
        }
        warn!(
            backup = %bak_hash,
            current = %pck_hash,
            "vanilla backup drifted from current RTV.pck, refreshing",
        );
    }

    copy_atomic(&pck, &bak)?;
    info!(hash = %pck_hash, path = %bak.display(), "wrote vanilla pck backup");
    Ok(pck_hash)
}

/// Copy `from` over `to` atomically (`<to>.tmp` + rename). Removes any
/// pre-existing target so Windows' rename semantics work.
pub fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|source| CrabbyError::io_at(parent.to_path_buf(), source))?;
    }
    let mut tmp_name = to
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = to.with_file_name(tmp_name);

    fs::copy(from, &tmp).map_err(|source| CrabbyError::io_at(tmp.clone(), source))?;
    if to.exists() {
        fs::remove_file(to).map_err(|source| CrabbyError::io_at(to.to_path_buf(), source))?;
    }
    fs::rename(&tmp, to).map_err(|source| CrabbyError::Bake {
        context: format!("renaming {} → {}", tmp.display(), to.display()),
        source: Box::new(source),
    })?;
    Ok(())
}

/// Restore `RTV.pck` from the vanilla backup. Atomic: the backup itself
/// is preserved; only `RTV.pck` is replaced. Errors if the backup is
/// missing.
pub fn restore_from_backup(game_dir: &Path) -> Result<()> {
    let bak = backup_path(game_dir);
    if !bak.is_file() {
        return Err(CrabbyError::Bake {
            context: format!("restore: backup {} not found", bak.display()),
            source: "vanilla backup must exist before restore".into(),
        });
    }
    let pck = pck_path(game_dir);
    copy_atomic(&bak, &pck)?;
    info!(path = %pck.display(), "restored vanilla RTV.pck from backup");
    Ok(())
}

#[cfg(test)]
mod tests;
