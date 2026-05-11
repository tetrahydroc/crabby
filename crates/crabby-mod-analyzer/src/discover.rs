//! Bridge between mod discovery (`crabby-config::discover_mods_for_config`)
//! and the analyzer. Pulls every enabled mod's `.gd` file contents
//! regardless of source layout (folder vs vmz/zip), feeds them through
//! [`crate::analyze_mod`], and surfaces a per-mod report.
//!
//! Used by `crabby-install` post-bake to log what each enabled mod
//! does. The same surface is also useful to the UI's planned
//! "Conflicts" view, and both surfaces consume `Vec<ModIntent>`.

use std::io::Read;
use std::path::Path;

use crabby_error::{CrabbyError, Result};
use crabby_manifest::{DiscoveredMod, ModSource};

use crate::{ModIntent, VanillaSchema, analyze_mod_with_schema};

/// Open a discovered mod's archive (vmz/zip) or folder root and yield
/// `(in-mod-relative-filename, source)` pairs for every `.gd` file.
///
/// Returns an empty vec when the mod has no scripts (asset-only mods,
/// `.tres`-only configurations). Errors only on IO / archive corruption,
/// since a mod that just doesn't ship `.gd` is a successful empty.
pub fn read_mod_scripts(mod_: &DiscoveredMod) -> Result<Vec<(String, String)>> {
    match mod_.source {
        ModSource::Folder => Ok(read_folder(&mod_.archive_path)),
        ModSource::Vmz | ModSource::Zip => read_archive(&mod_.archive_path),
    }
}

fn read_folder(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk_folder(root, root, &mut out);
    out
}

fn walk_folder(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        // Skip dot-dirs (`.git` checkouts, etc).
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        if path.is_dir() {
            walk_folder(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("gd") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if let Ok(s) = std::fs::read_to_string(&path) {
                out.push((rel, s));
            }
            // Non-utf8 .gd is theoretically possible, so skip silently
            // (the parser would reject anyway).
        }
    }
}

/// Read raw bytes from a single file inside a discovered mod's archive
/// or folder root, matched by its mod-relative path.
///
/// `mod_relative` is the path inside the mod (e.g.
/// `"overlays/Player.gd"`). Caller is responsible for stripping any
/// `res://` prefix the mod author may have written. Returns `Ok(None)`
/// when the file isn't found in the mod (not an error since overlay
/// resolution may probe optional paths); returns `Err` only on
/// archive-corruption or IO failure.
pub fn read_mod_file_bytes(
    mod_: &DiscoveredMod,
    mod_relative: &str,
) -> Result<Option<Vec<u8>>> {
    match mod_.source {
        ModSource::Folder => {
            let path = mod_.archive_path.join(mod_relative);
            match std::fs::read(&path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(CrabbyError::io_at(path, e)),
            }
        }
        ModSource::Vmz | ModSource::Zip => {
            let f = std::fs::File::open(&mod_.archive_path)
                .map_err(|s| CrabbyError::io_at(mod_.archive_path.to_path_buf(), s))?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| CrabbyError::Bake {
                context: format!("read mod archive {}", mod_.archive_path.display()),
                source: format!("zip: {e}").into(),
            })?;
            let mut entry = match z.by_name(mod_relative) {
                Ok(e) => e,
                Err(_) => return Ok(None),
            };
            if entry.is_dir() {
                return Ok(None);
            }
            let mut buf = Vec::with_capacity(entry.size() as usize);
            std::io::copy(&mut entry, &mut buf).map_err(|e| CrabbyError::Bake {
                context: format!(
                    "read entry {mod_relative} in {}",
                    mod_.archive_path.display()
                ),
                source: format!("{e}").into(),
            })?;
            Ok(Some(buf))
        }
    }
}

fn read_archive(path: &Path) -> Result<Vec<(String, String)>> {
    let f = std::fs::File::open(path).map_err(|s| CrabbyError::io_at(path.to_path_buf(), s))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| CrabbyError::Bake {
        context: format!("read mod archive {}", path.display()),
        source: format!("zip: {e}").into(),
    })?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| CrabbyError::Bake {
            context: format!("zip entry {i} in {}", path.display()),
            source: format!("{e}").into(),
        })?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !name.ends_with(".gd") {
            continue;
        }
        let mut buf = String::new();
        if entry.read_to_string(&mut buf).is_err() {
            continue; // non-utf8
        }
        out.push((name, buf));
    }
    Ok(out)
}

/// Analyze every installed mod (regardless of enabled state) without
/// a vanilla schema. The user wants to see latent conflicts on
/// disabled mods too (e.g. "if you turn this on, it'll fight X").
/// Use [`analyze_active_profile_with_schema`] when the bake just
/// produced a schema for sharper grading.
pub fn analyze_active_profile(game_dir: &Path) -> Result<Vec<ModIntent>> {
    analyze_active_profile_with_schema(game_dir, None)
}

/// Analyze every installed mod (enabled OR disabled) in the active
/// profile's mod_config, optionally with a vanilla schema for refined
/// severity grading on `take_over_path` / `set_script` sites.
///
/// Disabled-but-installed mods are included so the conflict surface
/// can warn about latent collisions before the user enables the mod.
/// Mods without `.gd` files (asset-only) get an empty `ModIntent`
/// included, and the count is meaningful even when there's nothing to
/// flag. Mods that fail to open surface as an empty intent + a
/// `tracing::warn!`; the bake doesn't bail.
pub fn analyze_active_profile_with_schema(
    game_dir: &Path,
    schema: Option<&VanillaSchema>,
) -> Result<Vec<ModIntent>> {
    analyze_active_profile_inner(game_dir, schema, /* enabled_only = */ false)
}

/// Same as [`analyze_active_profile`] but filtered to mods that are
/// **enabled** in the active profile. Used by the bake's wrapper-skip
/// pass: disabled mods don't ship, their hooks won't fire at runtime,
/// so wrappers for them shouldn't be kept.
///
/// The conflict UI keeps using the all-mods variant so latent
/// collisions on disabled mods still surface.
pub fn analyze_enabled_mods(game_dir: &Path) -> Result<Vec<ModIntent>> {
    analyze_active_profile_inner(game_dir, None, /* enabled_only = */ true)
}

/// Result of [`scan_active_profile`]: every analyzer artifact a UI boot
/// path needs, computed from a single mod-discovery walk.
///
/// Replaces the four-times-per-boot pattern of calling the per-purpose
/// analyzer + discovery functions by sharing one pass through the
/// on-disk mod set.
#[derive(Debug, Default, Clone)]
pub struct BootScan {
    /// Intents for every discovered mod (enabled OR disabled). The
    /// conflict UI consumes this so latent collisions on disabled
    /// mods still surface.
    pub all_intents: Vec<ModIntent>,
    /// IDs of mods enabled in the active profile, in the order they
    /// appear in the discovered list. Subset of `all_intents`.
    pub enabled_ids: Vec<String>,
    /// Active profile name as resolved from `mod_config.cfg`, or empty
    /// when no profile is configured. Surfaced so callers don't have to
    /// reload `ModConfig` to display it.
    pub active_profile: String,
}

impl BootScan {
    /// Iterate over the enabled-mod intents, in discovery order.
    /// Convenience for callers (e.g. the bake-status digest) that only
    /// care about the enabled subset.
    pub fn enabled_intents(&self) -> impl Iterator<Item = &ModIntent> {
        let set: std::collections::BTreeSet<&str> =
            self.enabled_ids.iter().map(String::as_str).collect();
        self.all_intents
            .iter()
            .filter(move |i| set.contains(i.mod_id.as_str()))
    }
}

/// Single-pass scan of the active profile. Walks every discovered mod
/// once, runs the analyzer over each, and partitions the result into
/// all-mods intents plus the enabled-ID subset.
///
/// Used by the launcher's boot path so the analyzer runs once instead
/// of twice (once for the conflict surface, once for the bake-status
/// digest). The per-mod GDScript walk is the dominant boot-time cost;
/// this halves it.
///
/// Returns an empty [`BootScan`] when no mods are discovered, never
/// errors on individual mod-read failures (those become empty intents
/// with a `tracing::warn!`, matching [`analyze_active_profile`] semantics).
pub fn scan_active_profile(game_dir: &Path) -> Result<BootScan> {
    let cfg = crabby_config::ModConfig::load_or_default(game_dir)?;
    let active_profile = cfg.active_profile.clone();
    let enabled_set: std::collections::BTreeSet<String> = cfg
        .profiles
        .get(&cfg.active_profile)
        .map(|p| {
            p.mods
                .iter()
                .filter(|(_, e)| e.enabled)
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();

    let discovered = crabby_config::discover_mods_for_config(game_dir, &cfg)?;
    let mut all_intents = Vec::with_capacity(discovered.len());
    let mut enabled_ids = Vec::new();
    for d in &discovered {
        if enabled_set.contains(&d.manifest.id) {
            enabled_ids.push(d.manifest.id.clone());
        }
        match read_mod_scripts(d) {
            Ok(files) => {
                let intent = analyze_mod_with_schema(
                    &d.manifest.id,
                    files.iter().map(|(n, s)| (n.as_str(), s.as_str())),
                    None,
                );
                all_intents.push(intent);
            }
            Err(e) => {
                tracing::warn!(
                    mod_id = %d.manifest.id,
                    archive = %d.archive_path.display(),
                    error = %e,
                    "analyzer: failed to read mod scripts; skipping",
                );
                all_intents.push(ModIntent {
                    mod_id: d.manifest.id.clone(),
                    ..Default::default()
                });
            }
        }
    }
    Ok(BootScan {
        all_intents,
        enabled_ids,
        active_profile,
    })
}

fn analyze_active_profile_inner(
    game_dir: &Path,
    schema: Option<&VanillaSchema>,
    enabled_only: bool,
) -> Result<Vec<ModIntent>> {
    let cfg = crabby_config::ModConfig::load_or_default(game_dir)?;
    let enabled_ids: std::collections::BTreeSet<String> = if enabled_only {
        cfg.profiles
            .get(&cfg.active_profile)
            .map(|p| {
                p.mods
                    .iter()
                    .filter(|(_, e)| e.enabled)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::collections::BTreeSet::new()
    };

    let discovered = crabby_config::discover_mods_for_config(game_dir, &cfg)?;
    let mut out = Vec::new();
    for d in &discovered {
        if enabled_only && !enabled_ids.contains(&d.manifest.id) {
            continue;
        }
        match read_mod_scripts(d) {
            Ok(files) => {
                let intent = analyze_mod_with_schema(
                    &d.manifest.id,
                    files.iter().map(|(n, s)| (n.as_str(), s.as_str())),
                    schema,
                );
                out.push(intent);
            }
            Err(e) => {
                tracing::warn!(
                    mod_id = %d.manifest.id,
                    archive = %d.archive_path.display(),
                    error = %e,
                    "analyzer: failed to read mod scripts; skipping",
                );
                out.push(ModIntent {
                    mod_id: d.manifest.id.clone(),
                    ..Default::default()
                });
            }
        }
    }
    Ok(out)
}

/// Render a one-line summary of a `ModIntent` for log output.
#[must_use]
pub fn one_line_summary(i: &ModIntent) -> String {
    use crate::{Resolvability, Severity};
    let static_hooks = i
        .hooks
        .iter()
        .filter(|h| h.resolvability == Resolvability::Static)
        .count();
    let mut hard = 0;
    let mut warn = 0;
    let mut info = 0;
    for c in &i.classic_patterns {
        match c.severity {
            Severity::Hard => hard += 1,
            Severity::Warn => warn += 1,
            Severity::Info => info += 1,
        }
    }
    format!(
        "{:32}  files={:3}  hooks={:3} (static {:3})  reg={:3}  classic H/W/I={}/{}/{}",
        i.mod_id,
        i.files_scanned.len(),
        i.hooks.len(),
        static_hooks,
        i.registry_writes.len(),
        hard,
        warn,
        info,
    )
}
