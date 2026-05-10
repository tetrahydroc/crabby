//! `<game-dir>/.crabby/mod_index.cfg`, flat lookup of every enabled
//! mod's on-disk path, written by the launcher and read by the
//! runtime shim.
//!
//! Why it exists: at boot the shim needs to mount each enabled mod
//! by id. Without an index it has to scan every archive in `Mods/`
//! (and every extra root) opening + parsing each `mod.txt` to find
//! a matching id. That `O(N×M)` walk reads files for **disabled**
//! mods too, they sit in the same directory and only their parsed
//! id reveals they aren't the matching one. Pre-resolving
//! the id -> path map at write time means the shim opens *only*
//! enabled mods at boot.
//!
//! # Format
//!
//! Godot ConfigFile syntax (same shape as `mod_config.cfg` and the
//! rest of the `.crabby/` state). One section per mod id:
//!
//! ```ini
//! [crabby]
//! schema_version = 1
//!
//! [mod.faction-warfare]
//! path = "C:/.../Mods/FactionWarfare.vmz"
//! source = "vmz"
//! version = "2.0.4"
//! mtime = 1714850000
//!
//! [mod.doinkoink-mcm]
//! path = "C:/.../Mods/MCM.vmz"
//! source = "vmz"
//! version = "2.6.3"
//! mtime = 1714849000
//! ```
//!
//! Only enabled mods are written. Disabled mods sitting in `Mods/`
//! are absent from the index, by design, the shim never touches
//! them.
//!
//! # Staleness
//!
//! Launcher rebuilds the index on every action that could change
//! the enabled set or the on-disk archive set: enable/disable
//! toggle, install, uninstall, profile switch, Rescan. The shim
//! tolerates one source of drift: an enabled id whose archive moved
//! or was removed since the last index write. In that case the shim
//! falls back to a one-shot targeted scan for *that* id (and only
//! that id), then proceeds. Disabled mods are never opened on the
//! fallback path either.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crabby_error::{CrabbyError, Result};
use crabby_manifest::{DiscoveredMod, ModSource};

use crate::{ModConfig, SCHEMA_VERSION};

/// Relative path of `mod_index.cfg` within the game directory.
pub const MOD_INDEX_REL_PATH: &str = ".crabby/mod_index.cfg";

/// One indexed mod. Path is absolute on disk; metadata fields are
/// snapshotted at write time so the launcher / shim can detect drift
/// without reopening the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModIndexEntry {
    /// Absolute path of the archive (or folder root for folder mods).
    pub path: PathBuf,
    /// Storage kind label: `"vmz"`, `"zip"`, or `"folder"`.
    pub source: String,
    /// `[mod] version` from the archive's `mod.txt` at index time.
    pub version: String,
    /// Archive / folder mtime in unix seconds, for cheap staleness
    /// checks. `0` when metadata couldn't be read (rare; tolerated).
    pub mtime: i64,
    /// `[mod] name` from the manifest. Hoisted into the index so the
    /// runtime shim can sort by load order without re-parsing every
    /// enabled mod's `mod.txt` at boot. Empty when missing (rare).
    pub name: String,
    /// `[mod] priority` from the manifest (default 0). Lower values
    /// load first; ties broken by lowercase name then filename. Same
    /// hoist rationale as `name`.
    pub priority: i64,
    /// Absolute path to the pre-rewritten archive in
    /// `<game-dir>/.crabby/mod_cache/`. Populated by the launcher's
    /// background `mod_cache::rebuild_for_enabled` pass. Empty for
    /// folder mods (which can't be persistently cached) and as a
    /// transient state when an archive was just enabled but the
    /// async cache rebuild hasn't completed yet, the shim falls
    /// back to mounting the source archive directly in that case.
    pub cache_path: PathBuf,
}

/// Whole index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModIndex {
    /// Mod id -> entry. `BTreeMap` for deterministic on-disk ordering.
    pub entries: BTreeMap<String, ModIndexEntry>,
}

impl ModIndex {
    /// Absolute path of `mod_index.cfg` inside `game_dir`.
    #[must_use]
    pub fn path_in(game_dir: &Path) -> PathBuf {
        game_dir.join(MOD_INDEX_REL_PATH)
    }

    /// Load the index from `game_dir`. Returns an empty index when the
    /// file doesn't exist (shim treats absence the same as missing
    /// entries, falls back to scanning per-id).
    pub fn load_or_default(game_dir: &Path) -> Result<Self> {
        let path = Self::path_in(game_dir);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .map_err(|s| CrabbyError::io_at(path.clone(), s))?;
        parse(&text).map_err(|source| CrabbyError::Config {
            context: format!("parsing {}", path.display()),
            source,
        })
    }

    /// Write the index to `game_dir`, creating `.crabby/` if needed.
    pub fn save(&self, game_dir: &Path) -> Result<()> {
        let path = Self::path_in(game_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|s| CrabbyError::io_at(parent.to_path_buf(), s))?;
        }
        let rendered = render(self);
        fs::write(&path, rendered).map_err(|s| CrabbyError::io_at(path, s))?;
        Ok(())
    }
}

/// Build (or rebuild) the index for `cfg`'s active profile against
/// the mods discovered on disk. Only entries for **enabled** mods
/// are included; disabled mods are excluded by design.
///
/// `discovered` is what `discover_mods_for_config` returned. Pass it
/// in rather than scanning here so the launcher can reuse the scan
/// for other UI work.
#[must_use]
pub fn build(cfg: &ModConfig, discovered: &[DiscoveredMod]) -> ModIndex {
    let by_id: BTreeMap<&str, &DiscoveredMod> =
        discovered.iter().map(|m| (m.manifest.id.as_str(), m)).collect();
    let mut entries = BTreeMap::new();
    let Some(profile) = cfg.active_profile() else {
        return ModIndex { entries };
    };
    for (id, mod_entry) in &profile.mods {
        if !mod_entry.enabled {
            continue;
        }
        let Some(d) = by_id.get(id.as_str()) else {
            // Enabled but not on disk, skip with no entry. Shim's
            // fallback path will scan and either find it or warn.
            continue;
        };
        let mtime = d
            .archive_path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        entries.insert(
            id.clone(),
            ModIndexEntry {
                path: d.archive_path.clone(),
                source: source_label(d.source).to_owned(),
                version: d.manifest.version.clone(),
                mtime,
                name: d.manifest.name.clone(),
                // Per-profile priority override wins over the manifest's
                // declared value (manifest is the author's default; the
                // override is the user's per-install adjustment).
                priority: mod_entry.priority_override.unwrap_or(d.manifest.priority),
                // Cache_path filled in by `rebuild_and_save` once it
                // has the game_dir; `build()` stays filesystem-pure for
                // testability.
                cache_path: PathBuf::new(),
            },
        );
    }
    ModIndex { entries }
}

/// Convenience: load the active config + discover + write the index.
/// Single call site for "the launcher just made a change that could
/// affect what the runtime should mount; refresh."
///
/// Does NOT rebuild the pre-rewritten archive cache, that runs
/// asynchronously from the UI layer (see `mod_cache::rebuild_for_enabled`)
/// so it doesn't block UI threads. Bake-status checks already live behind
/// their own debouncer, so callers can rely on this being fast (just
/// scans archive headers and writes ~500 bytes per mod to disk).
pub fn rebuild_and_save(game_dir: &Path) -> Result<ModIndex> {
    let cfg = ModConfig::load_or_default(game_dir)?;
    let discovered = crate::discover_mods_for_config(game_dir, &cfg)?;
    rebuild_and_save_from_discovered(game_dir, &cfg, &discovered)
}

/// Variant of [`rebuild_and_save`] that takes a pre-loaded config and
/// pre-discovered mod list. Used by the launcher boot path so the
/// archive-walk runs once across all four boot consumers (mod-tab
/// rows, conflict scan, bake-status, mod-index) instead of four times.
pub fn rebuild_and_save_from_discovered(
    game_dir: &Path,
    cfg: &ModConfig,
    discovered: &[crabby_manifest::DiscoveredMod],
) -> Result<ModIndex> {
    let mut index = build(cfg, discovered);
    // Backfill cache_path now that game_dir is known. `build()`
    // intentionally leaves cache_path empty so it stays testable
    // without filesystem deps.
    for (id, entry) in index.entries.iter_mut() {
        if entry.source == "folder" {
            // Folder mods aren't cached; leave path empty.
            continue;
        }
        entry.cache_path = crate::mod_cache::cache_path_for(game_dir, id, entry.mtime);
    }
    index.save(game_dir)?;
    Ok(index)
}

fn source_label(s: ModSource) -> &'static str {
    match s {
        ModSource::Vmz => "vmz",
        ModSource::Zip => "zip",
        ModSource::Folder => "folder",
    }
}

// ---- Internal: render + parse ----------------------------------------------
//
// The format is a tiny subset of Godot ConfigFile (key=value lines under
// bracketed sections, double-quoted strings with `\\` and `\"` escapes).
// Hand-rolled here because cfg_io is tied to ModConfig's specific shape;
// keeping mod_index self-contained avoids cross-coupling.

fn render(idx: &ModIndex) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "[crabby]");
    let _ = writeln!(out, "schema_version = {SCHEMA_VERSION}");
    out.push('\n');
    for (id, e) in &idx.entries {
        let _ = writeln!(out, "[mod.{id}]");
        let _ = writeln!(out, "path = \"{}\"", escape_quoted(&e.path.to_string_lossy()));
        let _ = writeln!(out, "source = \"{}\"", escape_quoted(&e.source));
        let _ = writeln!(out, "version = \"{}\"", escape_quoted(&e.version));
        let _ = writeln!(out, "mtime = {}", e.mtime);
        let _ = writeln!(out, "name = \"{}\"", escape_quoted(&e.name));
        let _ = writeln!(out, "priority = {}", e.priority);
        // Cache path may be empty (folder mods or not-yet-cached
        // archives); always emit so the on-disk schema is uniform.
        let _ = writeln!(
            out,
            "cache_path = \"{}\"",
            escape_quoted(&e.cache_path.to_string_lossy()),
        );
        out.push('\n');
    }
    out
}

fn parse(text: &str) -> std::result::Result<ModIndex, Box<dyn std::error::Error + Send + Sync>> {
    let mut current_section: Option<String> = None;
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for raw_line in text.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let Some(name) = rest.strip_suffix(']') else {
                return Err(format!("malformed section header: {raw_line:?}").into());
            };
            current_section = Some(name.trim().to_owned());
            sections.entry(name.trim().to_owned()).or_default();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("expected `key = value`: {raw_line:?}").into());
        };
        let Some(section) = current_section.as_deref() else {
            return Err(format!("key outside any section: {raw_line:?}").into());
        };
        sections
            .entry(section.to_owned())
            .or_default()
            .insert(k.trim().to_owned(), v.trim().to_owned());
    }

    let mut schema = 0u32;
    let mut entries: BTreeMap<String, ModIndexEntry> = BTreeMap::new();
    for (section, kv) in &sections {
        if section == "crabby" {
            if let Some(v) = kv.get("schema_version") {
                schema = v.parse().unwrap_or(0);
            }
            continue;
        }
        let Some(id) = section.strip_prefix("mod.") else {
            continue;
        };
        let Some(path) = kv.get("path").and_then(|v| strip_quotes(v)) else {
            continue;
        };
        let source = kv
            .get("source")
            .and_then(|v| strip_quotes(v))
            .unwrap_or_else(|| "vmz".to_owned());
        let version = kv
            .get("version")
            .and_then(|v| strip_quotes(v))
            .unwrap_or_default();
        let mtime: i64 = kv.get("mtime").and_then(|v| v.parse().ok()).unwrap_or(0);
        let name = kv
            .get("name")
            .and_then(|v| strip_quotes(v))
            .unwrap_or_else(|| id.to_owned());
        let priority: i64 = kv.get("priority").and_then(|v| v.parse().ok()).unwrap_or(0);
        let cache_path = kv
            .get("cache_path")
            .and_then(|v| strip_quotes(v))
            .map(PathBuf::from)
            .unwrap_or_default();
        entries.insert(
            id.to_owned(),
            ModIndexEntry {
                path: PathBuf::from(path),
                source,
                version,
                mtime,
                name,
                priority,
                cache_path,
            },
        );
    }
    if schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported mod_index schema_version {schema} (expected {SCHEMA_VERSION})"
        )
        .into());
    }
    Ok(ModIndex { entries })
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn strip_quotes(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find(['#', ';']) {
        // Tolerate `#` / `;` inside quoted strings, only strip when
        // the comment marker is outside any quote span.
        let mut in_quote = false;
        let mut esc = false;
        for (i, c) in line.char_indices() {
            if in_quote {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_quote = false;
                }
            } else if c == '"' {
                in_quote = true;
            } else if c == '#' || c == ';' {
                return &line[..i];
            }
        }
        let _ = idx;
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_round_trips() {
        let idx = ModIndex::default();
        let rendered = render(&idx);
        let parsed = parse(&rendered).expect("parse empty");
        assert!(parsed.entries.is_empty());
    }

    #[test]
    fn single_entry_round_trips() {
        let mut idx = ModIndex::default();
        idx.entries.insert(
            "test-mod".into(),
            ModIndexEntry {
                path: PathBuf::from("/games/RTV/Mods/Test.vmz"),
                source: "vmz".into(),
                version: "1.0.0".into(),
                mtime: 1_700_000_000,
                name: "Test Mod".into(),
                priority: 0,
                cache_path: PathBuf::new(),
            },
        );
        let rendered = render(&idx);
        let parsed = parse(&rendered).expect("parse single");
        assert_eq!(parsed, idx);
    }

    #[test]
    fn paths_with_quotes_and_backslashes_round_trip() {
        let mut idx = ModIndex::default();
        idx.entries.insert(
            "weird".into(),
            ModIndexEntry {
                path: PathBuf::from(r#"C:\Users\name "with quotes"\Mods\X.vmz"#),
                source: "vmz".into(),
                version: "0.1".into(),
                mtime: 1,
                name: "weird".into(),
                priority: 0,
                cache_path: PathBuf::new(),
            },
        );
        let rendered = render(&idx);
        let parsed = parse(&rendered).expect("parse weird");
        assert_eq!(parsed, idx);
    }

    #[test]
    fn unknown_schema_rejected() {
        let bad = "[crabby]\nschema_version = 999\n";
        let err = parse(bad).expect_err("schema mismatch");
        assert!(format!("{err}").contains("999"));
    }

    #[test]
    fn build_excludes_disabled_mods() {
        use crabby_manifest::{ModManifest, ModSource};
        let mut cfg = ModConfig::default_fresh();
        cfg.active_profile_mut().mods.insert(
            "kept".into(),
            crate::ModEntry { enabled: true, version: "1.0".into(), priority_override: None },
        );
        cfg.active_profile_mut().mods.insert(
            "dropped".into(),
            crate::ModEntry { enabled: false, version: "1.0".into(), priority_override: None },
        );
        let mk = |id: &str| DiscoveredMod {
            archive_path: PathBuf::from(format!("/Mods/{id}.vmz")),
            source: ModSource::Vmz,
            manifest: ModManifest {
                id: id.into(),
                name: id.into(),
                version: "1.0".into(),
                priority: 0,
                autoloads: Vec::new(),
                extra_sections: BTreeMap::new(),
            },
        };
        let discovered = vec![mk("kept"), mk("dropped")];
        let idx = build(&cfg, &discovered);
        assert!(idx.entries.contains_key("kept"));
        assert!(!idx.entries.contains_key("dropped"));
    }

    #[test]
    fn build_skips_enabled_with_no_disk_match() {
        let mut cfg = ModConfig::default_fresh();
        cfg.active_profile_mut().mods.insert(
            "ghost".into(),
            crate::ModEntry { enabled: true, version: "1.0".into(), priority_override: None },
        );
        let idx = build(&cfg, &[]);
        assert!(idx.entries.is_empty());
    }
}
