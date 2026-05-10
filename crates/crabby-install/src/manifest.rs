//! Install manifest, the JSON record of what crabby placed in the game dir.
//!
//! The manifest lets three things work reliably:
//!
//! - **First-run detection**: no manifest means install is needed.
//!   Manifest with a different crabby version triggers a reinstall
//!   (schema or template may have drifted).
//! - **Uninstall precision**: remove only the recorded files. If the
//!   user put a file next to a crabby file by the same name, uninstall
//!   leaves it alone (only recorded paths are removed).
//! - **Doctor diagnosis**: compare the manifest's expected files against
//!   what's actually on disk. Missing files trigger reinstall; extra
//!   files are safe and ignored.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crabby_bake::BakeKey;
use crabby_error::{CrabbyError, Result};
use serde::{Deserialize, Serialize};

use crate::artifacts::{MANIFEST_DIR, MANIFEST_FILE_NAME};

/// Schema version of the manifest itself. Bump when the on-disk shape
/// changes in a non-backwards-compatible way.
///
/// **v2**: added `vanilla_pck_hash`, `last_baked_pck_hash` for the
/// PCK-rewrite install path. Both default to `None` when reading a v1
/// manifest, so older installs migrate transparently; the next
/// install/bake populates them.
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;

/// JSON record of a completed install.
///
/// Paths are stored relative to the game directory so manifests remain
/// valid after the install is moved (e.g. Steam library relocation).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallManifest {
    /// Manifest schema version, not the same as crabby version.
    pub schema_version: u32,
    /// Bake key identifying the pack currently on disk. Compare against a
    /// fresh `BakeKey::from_pck` on re-run to decide whether to rebake.
    pub bake_key: BakeKey,
    /// Unix timestamp (seconds since epoch) of the last successful install.
    pub installed_at: u64,
    /// Game-dir-relative paths of every file crabby wrote.
    pub placed_files: Vec<String>,
    /// Game-dir-relative path to the `override.cfg` backup, if crabby took
    /// one during install. `None` means no prior `override.cfg` existed.
    pub override_cfg_backup: Option<String>,
    /// SHA-256 of the vanilla `RTV.pck` that was backed up. Compared
    /// against the current `RTV.pck.vanilla.bak` on each install to
    /// detect Steam-update drift. `None` means PCK rewrite hasn't been
    /// used yet (legacy side-pack-only install) or the backup hasn't
    /// been established.
    #[serde(default)]
    pub vanilla_pck_hash: Option<String>,
    /// SHA-256 of the modified `RTV.pck` produced by the last bake.
    /// Lets the installer recognize "is the on-disk PCK ours or
    /// vanilla?" without re-baking. `None` for installs that haven't
    /// yet rewritten the PCK.
    #[serde(default)]
    pub last_baked_pck_hash: Option<String>,
}

impl InstallManifest {
    /// Construct a fresh manifest stamped with the supplied bake key and
    /// the current system time.
    #[must_use]
    pub fn fresh(bake_key: BakeKey) -> Self {
        let installed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            bake_key,
            installed_at,
            placed_files: Vec::new(),
            override_cfg_backup: None,
            vanilla_pck_hash: None,
            last_baked_pck_hash: None,
        }
    }

    /// Path (absolute) where the manifest lives inside `game_dir`.
    #[must_use]
    pub fn path_in(game_dir: &Path) -> PathBuf {
        game_dir.join(MANIFEST_DIR).join(MANIFEST_FILE_NAME)
    }

    /// Load an existing manifest from `game_dir`. Returns `Ok(None)` when
    /// the manifest file doesn't exist (no prior install).
    pub fn load(game_dir: &Path) -> Result<Option<Self>> {
        let path = Self::path_in(game_dir);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|source| CrabbyError::io_at(path.clone(), source))?;
        let parsed: Self = serde_json::from_slice(&bytes).map_err(|source| CrabbyError::Bake {
            context: format!("parsing manifest {}", path.display()),
            source: Box::new(source),
        })?;
        Ok(Some(parsed))
    }

    /// Write the manifest under `game_dir`. Creates `MANIFEST_DIR` if missing.
    pub fn save(&self, game_dir: &Path) -> Result<()> {
        let dir = game_dir.join(MANIFEST_DIR);
        fs::create_dir_all(&dir).map_err(|source| CrabbyError::io_at(dir.clone(), source))?;
        let path = Self::path_in(game_dir);
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| CrabbyError::Bake {
            context: "serializing install manifest".into(),
            source: Box::new(source),
        })?;
        fs::write(&path, bytes).map_err(|source| CrabbyError::io_at(path, source))?;
        Ok(())
    }

    /// Remove the manifest file + its containing directory if empty.
    pub fn delete(game_dir: &Path) -> Result<()> {
        let path = Self::path_in(game_dir);
        if path.is_file() {
            fs::remove_file(&path).map_err(|source| CrabbyError::io_at(path, source))?;
        }
        let dir = game_dir.join(MANIFEST_DIR);
        if dir.is_dir()
            && fs::read_dir(&dir)
                .ok()
                .is_some_and(|mut it| it.next().is_none())
        {
            fs::remove_dir(&dir).map_err(|source| CrabbyError::io_at(dir, source))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "crabby-manifest-{tag}-{}-{}",
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
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn load_returns_none_when_missing() {
        let tmp = TempDir::new("missing");
        assert!(InstallManifest::load(&tmp.path).unwrap().is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new("roundtrip");
        let mut m = InstallManifest::fresh(BakeKey::new("0.1.0", 1000, 500, ""));
        m.placed_files.push("crabby_shim.gd".into());
        m.placed_files.push(".crabby/install.json".into());
        m.save(&tmp.path).unwrap();

        let loaded = InstallManifest::load(&tmp.path)
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, m);
    }

    #[test]
    fn delete_removes_manifest_and_empty_dir() {
        let tmp = TempDir::new("delete");
        InstallManifest::fresh(BakeKey::new("0.1.0", 1000, 500, ""))
            .save(&tmp.path)
            .unwrap();
        assert!(tmp.path.join(MANIFEST_DIR).is_dir());
        InstallManifest::delete(&tmp.path).unwrap();
        assert!(!tmp.path.join(MANIFEST_DIR).exists());
    }

    #[test]
    fn loads_legacy_v1_manifest_with_none_pck_fields() {
        // v1 shape: no vanilla_pck_hash / last_baked_pck_hash. The
        // serde defaults must populate them with None so older installs
        // upgrade transparently.
        let tmp = TempDir::new("v1-load");
        let v1_json = r#"{
            "schema_version": 1,
            "bake_key": "v=0.0.9 sz=100 mt=50",
            "installed_at": 1000,
            "placed_files": ["crabby_shim.gd"],
            "override_cfg_backup": null
        }"#;
        let path = InstallManifest::path_in(&tmp.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, v1_json).unwrap();

        let m = InstallManifest::load(&tmp.path).unwrap().expect("load");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.placed_files, vec!["crabby_shim.gd".to_owned()]);
        assert_eq!(m.vanilla_pck_hash, None);
        assert_eq!(m.last_baked_pck_hash, None);
    }

    #[test]
    fn delete_preserves_non_empty_manifest_dir() {
        let tmp = TempDir::new("keep-nonempty");
        fs::create_dir_all(tmp.path.join(MANIFEST_DIR)).unwrap();
        fs::write(tmp.path.join(MANIFEST_DIR).join("user_file.txt"), b"keep").unwrap();
        InstallManifest::fresh(BakeKey::new("0.1.0", 1000, 500, ""))
            .save(&tmp.path)
            .unwrap();

        InstallManifest::delete(&tmp.path).unwrap();
        // Manifest file gone, dir preserved because user_file.txt remains.
        assert!(tmp.path.join(MANIFEST_DIR).is_dir());
        assert!(tmp.path.join(MANIFEST_DIR).join("user_file.txt").is_file());
    }
}
