//! Read-only install inspector.
//!
//! Reports what the install looks like without changing anything. Useful
//! for user troubleshooting and for a future CLI `crabby doctor` command.

use std::path::Path;

use crabby_bake::BakeKey;
use crabby_error::Result;

use crate::game_dir::validate_game_dir;
use crate::manifest::InstallManifest;

/// High-level install status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    /// `game_dir` does not appear to be an RTV install, or validation failed.
    InvalidGameDir {
        /// Underlying validation-failure message, for user display.
        reason: String,
    },
    /// No manifest present, crabby hasn't been installed here.
    NotInstalled,
    /// Manifest present and bake key matches current inputs. All placed
    /// files accounted for.
    Current,
    /// Manifest present but bake key differs from current inputs, so
    /// a reinstall would rebake.
    Stale,
    /// Manifest bake key matches but one or more placed files are missing
    /// on disk (user deletion, antivirus, etc.). Reinstall would repair.
    Drifted {
        /// Paths (game-dir-relative) the manifest expected but aren't on disk.
        missing: Vec<String>,
    },
}

/// Full doctor report, the status plus the manifest (when any) and the
/// freshly-computed bake key (when the game dir was valid).
#[derive(Debug, Clone)]
pub struct DoctorReport {
    /// Absolute path of the game dir that was inspected.
    pub game_dir: std::path::PathBuf,
    /// Status conclusion.
    pub status: InstallStatus,
    /// Manifest as read from disk. `None` if not installed or game dir invalid.
    pub manifest: Option<InstallManifest>,
    /// Current bake key computed from the game dir's PCK. `None` if game
    /// dir validation failed.
    pub current_bake_key: Option<BakeKey>,
}

/// Inspect `game_dir` without modifying anything.
pub fn doctor(game_dir: &Path, crabby_version: &str) -> Result<DoctorReport> {
    let game_dir_abs = game_dir.to_path_buf();

    if let Err(e) = validate_game_dir(game_dir) {
        return Ok(DoctorReport {
            game_dir: game_dir_abs,
            status: InstallStatus::InvalidGameDir {
                reason: e.to_string(),
            },
            manifest: None,
            current_bake_key: None,
        });
    }

    let pck_path = game_dir.join("RTV.pck");
    let current_key = BakeKey::from_pck(crabby_version, &pck_path)?;

    let Some(manifest) = InstallManifest::load(game_dir)? else {
        return Ok(DoctorReport {
            game_dir: game_dir_abs,
            status: InstallStatus::NotInstalled,
            manifest: None,
            current_bake_key: Some(current_key),
        });
    };

    if manifest.bake_key != current_key {
        return Ok(DoctorReport {
            game_dir: game_dir_abs,
            status: InstallStatus::Stale,
            manifest: Some(manifest),
            current_bake_key: Some(current_key),
        });
    }

    let missing: Vec<String> = manifest
        .placed_files
        .iter()
        .filter(|rel| !game_dir.join(rel).is_file())
        .cloned()
        .collect();

    let status = if missing.is_empty() {
        InstallStatus::Current
    } else {
        InstallStatus::Drifted { missing }
    };

    Ok(DoctorReport {
        game_dir: game_dir_abs,
        status,
        manifest: Some(manifest),
        current_bake_key: Some(current_key),
    })
}
