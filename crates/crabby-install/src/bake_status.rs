//! "Is the baked PCK current?", the single source of truth for the
//! launcher's Launch button to decide between "Launch" and "Bake &
//! Launch."
//!
//! Computed by:
//! 1. Reading the install manifest (`.crabby/install.json`).
//! 2. Computing what the bake key *would* be against the vanilla
//!    backup + the active profile's enabled-mods digest.
//! 3. Comparing.
//!
//! Cheap enough to run on every Refresh: no PCK reads, just an
//! analyzer pass over enabled mods (which the launcher does anyway
//! for its conflict UI).

use std::path::Path;

use crabby_bake::{BakeKey, mods_digest_from_kinds};
use crabby_error::Result;

use crate::manifest::InstallManifest;
use crate::pck_backup::backup_path;

/// Result of a single "is the bake current?" check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BakeStatus {
    /// No manifest on disk, meaning crabby hasn't been installed here
    /// yet. Launcher should disable Launch and offer Install / Bake.
    NotInstalled,
    /// Manifest exists, and the recorded bake key matches the
    /// currently-computed one, so the PCK reflects current enabled
    /// mods. Launch is safe.
    UpToDate,
    /// Manifest exists but the recorded bake key doesn't match.
    /// Mods were toggled (or the vanilla PCK changed) since the last
    /// bake. Launch should re-bake first.
    OutOfDate {
        /// Key produced now, for diagnostic display.
        expected: BakeKey,
        /// Key recorded in the manifest, for diagnostic display.
        actual: BakeKey,
    },
    /// Couldn't compute, e.g. game dir invalid, vanilla backup missing,
    /// analyzer failed. Caller decides how to surface; Launch should
    /// probably stay disabled.
    Unknown {
        /// Free-form reason for the UI / log.
        reason: String,
    },
}

impl BakeStatus {
    /// True only for [`BakeStatus::UpToDate`].
    #[must_use]
    pub fn is_up_to_date(&self) -> bool {
        matches!(self, Self::UpToDate)
    }

    /// True for any state that requires a bake before the game can
    /// launch with current intent: explicit OutOfDate, or absence of
    /// any prior install. NotInstalled still needs a bake; Unknown is
    /// treated as "don't auto-bake without context."
    #[must_use]
    pub fn needs_bake(&self) -> bool {
        matches!(self, Self::NotInstalled | Self::OutOfDate { .. })
    }
}

/// Compute the current [`BakeStatus`] for `game_dir`. Pure read; no
/// mutation, no install pipeline kicked. Safe to call on every
/// launcher Refresh.
///
/// `crabby_version` should match the value the launcher would pass to
/// [`crate::install`]. Key drift due to a crabby version bump is one
/// of the legitimate "out of date" signals.
pub fn bake_status(game_dir: &Path, crabby_version: &str) -> Result<BakeStatus> {
    let intents = match crabby_mod_analyzer::analyze_enabled_mods(game_dir) {
        Ok(v) => v,
        Err(e) => {
            return Ok(BakeStatus::Unknown {
                reason: format!("analyzer failed: {e}"),
            });
        }
    };
    bake_status_from_intents(game_dir, crabby_version, intents.iter())
}

/// Same as [`bake_status`] but consumes a precomputed iterator of
/// enabled-mod intents. Lets boot paths that already ran the analyzer
/// (e.g. via [`crabby_mod_analyzer::scan_active_profile`]) skip the
/// duplicate per-mod GDScript walk - the dominant cost on first launch.
///
/// `intents` must be the **enabled subset** in the active profile.
/// Passing all-mods intents would diverge from what `install()` does
/// and produce a digest that never matches the manifest key.
pub fn bake_status_from_intents<'a, I>(
    game_dir: &Path,
    crabby_version: &str,
    intents: I,
) -> Result<BakeStatus>
where
    I: IntoIterator<Item = &'a crabby_mod_analyzer::ModIntent>,
{
    let Some(manifest) = InstallManifest::load(game_dir)? else {
        return Ok(BakeStatus::NotInstalled);
    };

    let backup = backup_path(game_dir);
    if !backup.exists() {
        return Ok(BakeStatus::Unknown {
            reason: format!(
                "vanilla pck backup missing at {}; install needed before launch",
                backup.display(),
            ),
        });
    }

    let intents: Vec<&crabby_mod_analyzer::ModIntent> = intents.into_iter().collect();
    let kinds = crabby_mod_analyzer::collect_hooked_method_kinds_from_refs(intents.iter().copied());
    // Digest inputs match `install()`'s exactly so the launcher's
    // "is bake current?" answer doesn't diverge from what install would
    // actually do. The enabled-IDs section catches profile swaps where
    // the new profile's hook footprint coincidentally matches.
    let enabled_ids: Vec<&str> = intents.iter().map(|i| i.mod_id.as_str()).collect();
    let digest = mods_digest_from_kinds(
        kinds
            .iter()
            .map(|(k, v)| (k.as_str(), [v.pre, v.post, v.callback, v.replace])),
        enabled_ids,
    );

    let expected = BakeKey::from_pck_with_mods(crabby_version, &backup, &digest)?;
    if expected == manifest.bake_key {
        Ok(BakeStatus::UpToDate)
    } else {
        Ok(BakeStatus::OutOfDate {
            expected,
            actual: manifest.bake_key,
        })
    }
}
