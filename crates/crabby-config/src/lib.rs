//! `mod_config.cfg`, which mods are present, enabled, and grouped into
//! named profiles.
//!
//! The file lives at `<game-dir>/.crabby/mod_config.cfg`. Format is
//! Godot's `ConfigFile` (INI-with-typed-values), so the `GDScript` shim
//! reads it natively without a separate parser on that side.
//!
//! # Schema
//!
//! ```cfg
//! [crabby]
//! schema_version = 1
//! active_profile = "default"
//!
//! [profile.default]
//! hold-breath = { "enabled": true, "version": "1.0.3" }
//! rtv-lib-test-a = { "enabled": false, "version": "0.2.0" }
//!
//! [profile.testing]
//! hold-breath = { "enabled": true, "version": "1.0.3" }
//! ```
//!
//! Per mod, `enabled` (boolean) and `version` (string, captured at
//! enable time) are recorded. `disable` flips `enabled` to `false` but
//! keeps the entry so re-enabling later doesn't re-prompt for version
//! selection.
//!
//! # Discovery
//!
//! [`discover_mods`] scans `<game-dir>/Mods/*.{vmz,zip}` (flat, no
//! recursion, vostok convention) and parses each archive's `mod.txt`
//! via [`crabby_manifest`]. Archives with a missing or unparseable
//! `mod.txt` are skipped with a warning, crabby never refuses to
//! start because of one broken archive.

#![deny(missing_docs)]

mod cfg_io;
pub mod keycode;
pub mod mcm;
pub mod mod_cache;
pub mod mod_index;
pub mod saves;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};
use crabby_manifest::{DiscoveredMod, ModManifest, ModSource};
use tracing::{debug, warn};

/// Relative path of `mod_config.cfg` within the game directory.
pub const MOD_CONFIG_REL_PATH: &str = ".crabby/mod_config.cfg";

/// Subdirectory of the game directory scanned by [`discover_mods`].
pub const MODS_DIR_NAME: &str = "Mods";

/// Current config schema version. Bumped when the on-disk shape changes
/// in a non-backwards-compatible way.
pub const SCHEMA_VERSION: u32 = 1;

/// Default profile name created when [`ModConfig::default_fresh`] is used.
pub const DEFAULT_PROFILE: &str = "default";

/// One mod's entry inside a profile. `version` is captured at enable
/// time and surfaced in drift warnings; `enabled` is what the loader
/// actually checks at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    /// Whether the loader should mount this mod at runtime.
    pub enabled: bool,
    /// `[mod] version` from the archive's `mod.txt` as of enable time.
    pub version: String,
    /// User-specified load-order priority that overrides the manifest's
    /// `[mod] priority=`. Lower runs first; ties broken by name then
    /// filename in `_compare_load_order`. `None` = inherit from manifest.
    /// Per-profile so the same mod can have different ordering across
    /// playthroughs without touching the archive.
    pub priority_override: Option<i64>,
}

/// A named collection of mod entries, keyed by mod id.
///
/// `BTreeMap` for deterministic on-disk ordering, re-saving the same
/// config produces byte-identical output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    /// Mod id -> entry.
    pub mods: BTreeMap<String, ModEntry>,
}

/// One extra root the launcher (and the runtime shim) should scan for
/// mods, on top of the canonical `<game-dir>/Mods/`. `dev = true` roots
/// override same-id mods from `Mods/` and from non-dev roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootEntry {
    /// Filesystem path. Stored as the user typed/picked it; resolved
    /// relative to the game-dir's parent at scan time if it's relative.
    pub path: PathBuf,
    /// Dev-precedence flag. Multiple dev roots are searched in the
    /// order they appear in the config.
    pub dev: bool,
}

/// Top-level config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModConfig {
    /// Schema version. Mismatch is a hard error; auto-migration is not done.
    pub schema_version: u32,
    /// Name of the profile whose enabled mods get loaded at runtime.
    pub active_profile: String,
    /// Profile name -> profile.
    pub profiles: BTreeMap<String, Profile>,
    /// Extra mod-source roots (beyond `<game-dir>/Mods/`). Order is
    /// preserved on round-trip so the dev-vs-non-dev precedence rules
    /// inside each layer stay deterministic.
    pub extra_roots: Vec<RootEntry>,
}

impl ModConfig {
    /// Construct a fresh config with one empty `"default"` profile.
    #[must_use]
    pub fn default_fresh() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert(DEFAULT_PROFILE.to_owned(), Profile::default());
        Self {
            schema_version: SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE.to_owned(),
            profiles,
            extra_roots: Vec::new(),
        }
    }

    /// Absolute path of `mod_config.cfg` inside `game_dir`.
    #[must_use]
    pub fn path_in(game_dir: &Path) -> PathBuf {
        game_dir.join(MOD_CONFIG_REL_PATH)
    }

    /// Load the config from `game_dir`. Returns [`Self::default_fresh`]
    /// when the file doesn't exist; a first-run game dir shouldn't error.
    pub fn load_or_default(game_dir: &Path) -> Result<Self> {
        let path = Self::path_in(game_dir);
        if !path.is_file() {
            return Ok(Self::default_fresh());
        }
        let text = fs::read_to_string(&path).map_err(|s| CrabbyError::io_at(path.clone(), s))?;
        let parsed = cfg_io::parse(&text).map_err(|source| CrabbyError::Config {
            context: format!("parsing {}", path.display()),
            source,
        })?;
        if parsed.schema_version != SCHEMA_VERSION {
            return Err(CrabbyError::Config {
                context: format!(
                    "unsupported schema_version {} in {} (expected {SCHEMA_VERSION})",
                    parsed.schema_version,
                    path.display(),
                ),
                source: "schema mismatch".into(),
            });
        }
        if !parsed.profiles.contains_key(&parsed.active_profile) {
            return Err(CrabbyError::Config {
                context: format!(
                    "active_profile {:?} in {} refers to an undefined profile",
                    parsed.active_profile,
                    path.display(),
                ),
                source: "active_profile must name an existing [profile.<name>] section".into(),
            });
        }
        Ok(parsed)
    }

    /// Write the config to `game_dir`, creating `.crabby/` if needed.
    pub fn save(&self, game_dir: &Path) -> Result<()> {
        let path = Self::path_in(game_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|s| CrabbyError::io_at(parent.to_path_buf(), s))?;
        }
        let rendered = cfg_io::render(self);
        fs::write(&path, rendered).map_err(|s| CrabbyError::io_at(path, s))?;
        Ok(())
    }

    /// Mutable access to the active profile. Creates the entry if
    /// missing (shouldn't happen after load validation, but saves a
    /// double-check at every mutation site).
    pub fn active_profile_mut(&mut self) -> &mut Profile {
        let name = self.active_profile.clone();
        self.profiles.entry(name).or_default()
    }

    /// Read-only view of the active profile.
    #[must_use]
    pub fn active_profile(&self) -> Option<&Profile> {
        self.profiles.get(&self.active_profile)
    }
}

/// Scan `<game_dir>/Mods/*.{vmz,zip}` and return every archive whose
/// `mod.txt` parses cleanly.
///
/// Archives without a readable `mod.txt` are **not** included in the
/// returned list, but are logged at `warn` level; crabby shouldn't
/// refuse to start because a single mod archive is corrupt.
pub fn discover_mods(game_dir: &Path) -> Result<Vec<DiscoveredMod>> {
    let mods_dir = game_dir.join(MODS_DIR_NAME);
    scan_root(&mods_dir)
}

/// Discover mods across the canonical `<game-dir>/Mods/` plus any
/// extra roots the user configured. `extra_roots` is an iterator of
/// `(path, dev)` pairs.
///
/// Resolution per id (first hit wins):
/// 1. dev roots (in the order they appear in `extra_roots`)
/// 2. game-dir `Mods/`
/// 3. non-dev extra roots (in order)
///
/// Dropped duplicates are logged at `debug` level so a `RUST_LOG=debug`
/// session can show what was overridden by what. The returned vec is
/// sorted by id.
pub fn discover_mods_with_roots<'a, I>(
    game_dir: &Path,
    extra_roots: I,
) -> Result<Vec<DiscoveredMod>>
where
    I: IntoIterator<Item = (&'a Path, bool)>,
{
    let mut dev_layer: Vec<DiscoveredMod> = Vec::new();
    let mut other_layer: Vec<DiscoveredMod> = Vec::new();
    for (root, dev) in extra_roots {
        let scanned = scan_root(root)?;
        if dev {
            dev_layer.extend(scanned);
        } else {
            other_layer.extend(scanned);
        }
    }
    let game_layer = scan_root(&game_dir.join(MODS_DIR_NAME))?;

    let mut by_id: BTreeMap<String, DiscoveredMod> = BTreeMap::new();
    // Lower-precedence first so higher-precedence overwrites.
    for m in other_layer {
        if let Some(prev) = by_id.insert(m.manifest.id.clone(), m) {
            debug!(id = %prev.manifest.id, path = %prev.archive_path.display(), "shadowed by later non-dev root");
        }
    }
    for m in game_layer {
        if let Some(prev) = by_id.insert(m.manifest.id.clone(), m) {
            debug!(id = %prev.manifest.id, path = %prev.archive_path.display(), "shadowed by game-dir Mods/");
        }
    }
    for m in dev_layer {
        if let Some(prev) = by_id.insert(m.manifest.id.clone(), m) {
            debug!(id = %prev.manifest.id, path = %prev.archive_path.display(), "shadowed by dev root");
        }
    }

    Ok(by_id.into_values().collect())
}

/// Convenience: discover using the `extra_roots` declared in the
/// given [`ModConfig`]. The launcher and CLI both call this so they
/// see the same set of mods (with the same dev-precedence) as the
/// runtime shim will at boot.
pub fn discover_mods_for_config(
    game_dir: &Path,
    cfg: &ModConfig,
) -> Result<Vec<DiscoveredMod>> {
    let roots: Vec<(&Path, bool)> = cfg
        .extra_roots
        .iter()
        .map(|r| (r.path.as_path(), r.dev))
        .collect();
    discover_mods_with_roots(game_dir, roots)
}

/// Scan a single directory for mods. Used by both [`discover_mods`]
/// (with `<game-dir>/Mods/`) and [`discover_mods_with_roots`] (with
/// each configured root).
fn scan_root(dir: &Path) -> Result<Vec<DiscoveredMod>> {
    if !dir.is_dir() {
        debug!(path = %dir.display(), "scan: not a directory, skipping");
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|s| CrabbyError::io_at(dir.to_path_buf(), s))?;
    for entry in entries {
        let entry = entry.map_err(|s| CrabbyError::io_at(dir.to_path_buf(), s))?;
        let path = entry.path();

        if path.is_file() {
            let Some(source) = archive_source(&path) else {
                continue;
            };
            match ModManifest::read_from_archive(&path) {
                Ok(manifest) => out.push(DiscoveredMod {
                    archive_path: path,
                    source,
                    manifest,
                }),
                Err(e) => warn!(
                    archive = %path.display(),
                    error = %e,
                    "skipping archive: mod.txt missing or unparseable",
                ),
            }
        } else if path.is_dir() {
            // Folder mods: dev workflow where the modder edits files in
            // place. Must contain `mod.txt` at the folder root; anything
            // without it is silently skipped (might be a stray dir,
            // editor cache, etc.).
            if !path.join("mod.txt").is_file() {
                continue;
            }
            match ModManifest::read_from_dir(&path) {
                Ok(manifest) => out.push(DiscoveredMod {
                    archive_path: path,
                    source: ModSource::Folder,
                    manifest,
                }),
                Err(e) => warn!(
                    folder = %path.display(),
                    error = %e,
                    "skipping folder: mod.txt unparseable",
                ),
            }
        }
    }

    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(out)
}

/// Classify a file path into a [`ModSource`], or `None` if it isn't
/// a recognized mod archive extension.
fn archive_source(path: &Path) -> Option<ModSource> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("vmz") {
        Some(ModSource::Vmz)
    } else if ext.eq_ignore_ascii_case("zip") {
        Some(ModSource::Zip)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "crabby-config-{tag}-{}-{}",
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
    fn load_default_when_missing() {
        let tmp = TempDir::new("missing");
        let cfg = ModConfig::load_or_default(&tmp.path).unwrap();
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        assert_eq!(cfg.active_profile, DEFAULT_PROFILE);
        assert!(cfg.profiles.contains_key(DEFAULT_PROFILE));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new("roundtrip");
        let mut cfg = ModConfig::default_fresh();
        cfg.active_profile_mut().mods.insert(
            "hold-breath".into(),
            ModEntry {
                enabled: true,
                version: "1.0.3".into(),
                priority_override: None,
            },
        );
        cfg.active_profile_mut().mods.insert(
            "rtv-lib-test-a".into(),
            ModEntry {
                enabled: false,
                version: "0.2.0".into(),
                priority_override: None,
            },
        );
        cfg.save(&tmp.path).unwrap();

        let loaded = ModConfig::load_or_default(&tmp.path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let tmp = TempDir::new("schema");
        let path = ModConfig::path_in(&tmp.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "[crabby]\nschema_version = 999\nactive_profile = \"default\"\n\n[profile.default]\n",
        )
        .unwrap();
        let err = ModConfig::load_or_default(&tmp.path).unwrap_err();
        assert!(matches!(err, CrabbyError::Config { .. }));
    }

    #[test]
    fn active_profile_must_exist() {
        let tmp = TempDir::new("active-missing");
        let path = ModConfig::path_in(&tmp.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "[crabby]\nschema_version = 1\nactive_profile = \"nope\"\n\n[profile.default]\n",
        )
        .unwrap();
        let err = ModConfig::load_or_default(&tmp.path).unwrap_err();
        assert!(matches!(err, CrabbyError::Config { .. }));
    }

    #[test]
    fn discover_mods_returns_empty_when_no_dir() {
        let tmp = TempDir::new("no-mods-dir");
        assert!(discover_mods(&tmp.path).unwrap().is_empty());
    }
}
