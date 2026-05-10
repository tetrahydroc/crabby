//! Uninstall orchestrator.
//!
//! Reverses what [`install`](crate::install) did, using the manifest as
//! the authoritative list of what crabby owns. Anything the user dropped
//! in the game dir outside the manifest is left alone.

use std::fs;
use std::path::Path;

use crabby_error::{CrabbyError, Result};
use tracing::{info, warn};

use crate::artifacts::{OVERRIDE_CFG_BACKUP_NAME, OVERRIDE_CFG_NAME};
use crate::manifest::InstallManifest;
use crate::pck_backup::{backup_path, restore_from_backup};

/// Report returned by [`uninstall`].
#[derive(Debug, Clone)]
pub struct UninstallReport {
    /// Count of files deleted from `game_dir`.
    pub removed_file_count: usize,
    /// Whether an `override.cfg` backup was restored.
    pub restored_override_cfg: bool,
    /// Whether `RTV.pck` was restored from the vanilla backup.
    pub restored_pck: bool,
    /// Whether there was any manifest to uninstall. `false` means the call
    /// was a no-op.
    pub had_manifest: bool,
}

/// Uninstall crabby from `game_dir`.
///
/// If no manifest exists, returns a no-op report. Otherwise:
///
/// 1. If a vanilla PCK backup exists, restore `RTV.pck` from it.
///    Backup file itself is left in place (next install will reuse it).
/// 2. Delete every file listed in `manifest.placed_files` (if present).
/// 3. Restore the `override.cfg` backup if one was recorded, else
///    delete our `override.cfg` outright.
/// 4. Delete the manifest file itself and the `.crabby/` dir (if empty).
pub fn uninstall(game_dir: &Path) -> Result<UninstallReport> {
    let Some(manifest) = InstallManifest::load(game_dir)? else {
        return Ok(UninstallReport {
            removed_file_count: 0,
            restored_override_cfg: false,
            restored_pck: false,
            had_manifest: false,
        });
    };

    let restored_pck = if backup_path(game_dir).is_file() {
        restore_from_backup(game_dir)?;
        true
    } else {
        warn!("no vanilla PCK backup found; leaving RTV.pck as-is");
        false
    };

    let mut removed = 0usize;
    for rel in &manifest.placed_files {
        if rel.ends_with("install.json") {
            // Handled separately by InstallManifest::delete below.
            continue;
        }
        let abs = game_dir.join(rel);
        if abs.is_file() {
            fs::remove_file(&abs).map_err(|s| CrabbyError::io_at(abs.clone(), s))?;
            removed += 1;
            info!("removed {}", abs.display());
        } else {
            warn!(
                "expected {} to exist for uninstall, skipping",
                abs.display()
            );
        }
    }

    let override_path = game_dir.join(OVERRIDE_CFG_NAME);
    let restored = if let Some(backup_rel) = &manifest.override_cfg_backup {
        let backup_abs = game_dir.join(backup_rel);
        if backup_abs.is_file() {
            fs::rename(&backup_abs, &override_path)
                .map_err(|s| CrabbyError::io_at(override_path.clone(), s))?;
            info!("restored {}", override_path.display());
            true
        } else {
            warn!(
                "override.cfg backup {} missing; deleting crabby's override.cfg instead",
                backup_abs.display(),
            );
            if override_path.is_file() {
                fs::remove_file(&override_path)
                    .map_err(|s| CrabbyError::io_at(override_path.clone(), s))?;
            }
            false
        }
    } else {
        // No backup: crabby owned the override.cfg outright. Delete it.
        if override_path.is_file() {
            fs::remove_file(&override_path)
                .map_err(|s| CrabbyError::io_at(override_path.clone(), s))?;
            removed += 1;
        }
        false
    };

    InstallManifest::delete(game_dir)?;

    // Defense in depth: remove a lingering backup file even if it
    // wasn't restored from (e.g. restore failed above).
    let stray_backup = game_dir.join(OVERRIDE_CFG_BACKUP_NAME);
    if stray_backup.is_file() {
        fs::remove_file(&stray_backup).map_err(|s| CrabbyError::io_at(stray_backup.clone(), s))?;
    }

    Ok(UninstallReport {
        removed_file_count: removed,
        restored_override_cfg: restored,
        restored_pck,
        had_manifest: true,
    })
}
