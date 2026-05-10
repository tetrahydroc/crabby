//! Parse `mod.txt` into a typed `ModManifest`.
//!
//! The on-disk schema is INI-style for compatibility with vostok-mod-loader:
//!
//! ```ini
//! [mod]
//! name="Hold Breath"
//! id="hold-breath"
//! version="1.0.3"
//!
//! [autoload]
//! HoldBreathBoot="res://HoldBreath/Main.gd"
//! HoldBreathConfig="res://HoldBreath/Config.gd"
//!
//! [updates]
//! modworkshop=55938
//! ```
//!
//! Only `[mod]` and `[autoload]` are needed to load a mod; any other
//! sections are preserved in [`ModManifest::extra_sections`] for diagnostic
//! echoing but have no runtime meaning to crabby.
//!
//! # Error convention
//!
//! Parse + validation failures convert into [`CrabbyError::Manifest`].

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};

/// A single `[autoload]` entry from `mod.txt`.
///
/// Godot autoload rows look like `Name="*res://path/to/script.gd"`. The
/// leading `*` on the path means "singleton, auto-instance at startup."
/// The `*` is stripped for internal storage and re-emitted when registering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Autoload {
    /// Name the autoload is registered as (accessible as a global in `GDScript`).
    pub name: String,
    /// Resource path, without the leading `*` singleton marker.
    /// Typically `res://<mod_dir>/Script.gd`.
    pub path: String,
    /// Whether the original entry was marked as a singleton (`*` prefix).
    /// Almost always `true` in the wild; kept for accurate round-trip.
    pub singleton: bool,
}

/// Parsed `mod.txt` contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModManifest {
    /// Stable identifier from `[mod] id=`. Used as the mod's canonical
    /// name in `mod_config.toml` and in logs.
    pub id: String,
    /// Human-readable name from `[mod] name=`.
    pub name: String,
    /// Version string from `[mod] version=`.
    pub version: String,
    /// Load-order priority from `[mod] priority=` (default 0). Lower
    /// values load first; ties broken by lowercase mod_name then
    /// filename. Mirrors vostok-mod-loader's `_compare_load_order`.
    /// Mods like MCM that need to register class_names early use a
    /// negative priority (e.g. `priority=-100`).
    pub priority: i64,
    /// Declared autoloads under `[autoload]`, in declaration order.
    pub autoloads: Vec<Autoload>,
    /// Any unknown section → key/value map. Preserved so diagnostics can
    /// echo vendor-specific sections (e.g. `[updates]`) without crabby
    /// needing to know about them.
    pub extra_sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl ModManifest {
    /// Parse a `mod.txt` from raw bytes.
    ///
    /// Accepts both UTF-8 and latin-1-compatible input (round-trips
    /// through lossy UTF-8 rather than erroring; a stray non-ASCII
    /// character in a `name=` field shouldn't kill the load).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let text = String::from_utf8_lossy(bytes);
        Self::parse_str(&text)
    }

    /// Parse a `mod.txt` from an in-memory string.
    pub fn parse_str(text: &str) -> Result<Self> {
        let sections = parse_ini(text)?;

        let mod_section = sections.get("mod").ok_or_else(|| CrabbyError::Manifest {
            context: "mod.txt missing [mod] section".into(),
            source: "expected at least [mod] id=, name=, version=".into(),
        })?;

        let id = require_field(mod_section, "mod", "id")?;
        let name = require_field(mod_section, "mod", "name")?;
        let version = require_field(mod_section, "mod", "version")?;
        let priority: i64 = mod_section
            .get("priority")
            .and_then(|raw| raw.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);

        let autoloads = sections
            .get("autoload")
            .map(parse_autoloads)
            .unwrap_or_default();

        let mut extra_sections = BTreeMap::new();
        for (section, entries) in &sections {
            if section == "mod" || section == "autoload" {
                continue;
            }
            extra_sections.insert(section.clone(), entries.clone());
        }

        Ok(Self {
            id,
            name,
            version,
            priority,
            autoloads,
            extra_sections,
        })
    }

    /// Read and parse `mod.txt` at the root of a mod folder. Used for
    /// dev workflows where modders edit `Mods/<id>/` directly without
    /// re-zipping each iteration.
    ///
    /// # Errors
    ///
    /// [`CrabbyError::Io`] if the directory can't be read, or
    /// [`CrabbyError::Manifest`] if `mod.txt` is missing or unparseable.
    pub fn read_from_dir(dir_path: &Path) -> Result<Self> {
        let mod_txt_path = dir_path.join("mod.txt");
        let bytes = fs::read(&mod_txt_path).map_err(|s| CrabbyError::Manifest {
            context: format!("folder {} has no mod.txt at its root", dir_path.display(),),
            source: Box::new(s),
        })?;
        Self::parse(&bytes)
    }

    /// Read and parse the `mod.txt` at the root of a mod archive
    /// (`.vmz` or `.zip`).
    ///
    /// # Errors
    ///
    /// [`CrabbyError::Io`] if the archive can't be opened, or
    /// [`CrabbyError::Manifest`] if it lacks a readable `mod.txt`.
    pub fn read_from_archive(archive_path: &Path) -> Result<Self> {
        let bytes = fs::read(archive_path)
            .map_err(|s| CrabbyError::io_at(archive_path.to_path_buf(), s))?;
        let reader = Cursor::new(&bytes);
        let mut zip = zip::ZipArchive::new(reader).map_err(|e| CrabbyError::Manifest {
            context: format!("opening archive {}", archive_path.display()),
            source: Box::new(e),
        })?;
        let mut entry = zip.by_name("mod.txt").map_err(|e| CrabbyError::Manifest {
            context: format!(
                "archive {} has no mod.txt at its root",
                archive_path.display()
            ),
            source: Box::new(e),
        })?;
        let mut mod_txt_bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry
            .read_to_end(&mut mod_txt_bytes)
            .map_err(|s| CrabbyError::io_at(archive_path.to_path_buf(), s))?;
        Self::parse(&mod_txt_bytes)
    }
}

/// What kind of on-disk thing the mod was discovered as. Affects how
/// the runtime mounts it (zip → `load_resource_pack`; folder → no
/// runtime mount support yet but the UI surfaces it for dev visibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSource {
    /// `Mods/<name>.vmz` - vostok-mod-loader's renamed zip. Default
    /// shipping format for distributed mods.
    Vmz,
    /// `Mods/<name>.zip` - plain zip with the same internal layout
    /// as a vmz.
    Zip,
    /// `Mods/<name>/` - folder with `mod.txt` at the root and source
    /// files under it. Convenient for active development; the runtime
    /// loader doesn't currently mount folder mods.
    Folder,
}

impl ModSource {
    /// Short label for diagnostic logging and UI badges.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Vmz => "vmz",
            Self::Zip => "zip",
            Self::Folder => "folder",
        }
    }
}

/// On-disk summary of a mod discovered in `<game-dir>/Mods/`.
///
/// Pairs the storage path with the manifest so callers don't re-open
/// the source for routine identity lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMod {
    /// Absolute path of the archive or folder root. Field is named
    /// `archive_path` for historical reasons; for [`ModSource::Folder`]
    /// it's the directory containing `mod.txt`.
    pub archive_path: PathBuf,
    /// Source kind: archive vs folder.
    pub source: ModSource,
    /// Parsed manifest.
    pub manifest: ModManifest,
}

fn require_field(
    section: &BTreeMap<String, String>,
    section_name: &str,
    field: &str,
) -> Result<String> {
    section
        .get(field)
        .cloned()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CrabbyError::Manifest {
            context: format!("[{section_name}] missing required `{field}`"),
            source: format!("expected `{field}=\"...\"` in [{section_name}]").into(),
        })
}

fn parse_autoloads(section: &BTreeMap<String, String>) -> Vec<Autoload> {
    section
        .iter()
        .map(|(name, path)| {
            let (singleton, stripped) = path
                .strip_prefix('*')
                .map_or((false, path.as_str()), |s| (true, s));
            Autoload {
                name: name.clone(),
                path: stripped.to_owned(),
                singleton,
            }
        })
        .collect()
}

/// Parse an INI-style file into `section -> key -> value`.
///
/// Quote-handling matches vostok-mod-loader's `mod.txt` dialect:
///
/// - Values may be bare (`key=42`) or quoted (`key="hello"`).
/// - Quoted values have the surrounding `"` stripped; embedded quotes are
///   kept as-is (we never see escaped quotes in real mod.txt files).
/// - Comments start with `;` or `#` and run to end of line.
/// - Blank lines are ignored.
/// - Section headers are `[name]` on their own line.
fn parse_ini(text: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current = String::new(); // "" before the first section header

    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section.trim().clone_into(&mut current);
            sections.entry(current.clone()).or_default();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| CrabbyError::Manifest {
            context: format!("mod.txt line {}: expected `key=value`", lineno + 1),
            source: format!("got {line:?}").into(),
        })?;
        let key = key.trim().to_owned();
        let value = strip_quotes(value.trim()).to_owned();
        sections
            .entry(current.clone())
            .or_default()
            .insert(key, value);
    }

    Ok(sections)
}

fn strip_comment(line: &str) -> &str {
    // Only strip from an unquoted context; values may legitimately contain
    // `#` (URLs, hashes). mod.txt in practice never uses inline comments,
    // so a simple full-line check suffices.
    let trimmed = line.trim_start();
    if trimmed.starts_with(';') || trimmed.starts_with('#') {
        return "";
    }
    line
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD_BREATH_SAMPLE: &str = r#"[mod]
name="Hold Breath"
id="hold-breath"
version="1.0.3"

[autoload]
HoldBreathBoot="*res://HoldBreath/Main.gd"
HoldBreathConfig="*res://HoldBreath/Config.gd"

[updates]
modworkshop=55938
"#;

    #[test]
    fn parses_hold_breath_sample() {
        let m = ModManifest::parse_str(HOLD_BREATH_SAMPLE).unwrap();
        assert_eq!(m.id, "hold-breath");
        assert_eq!(m.name, "Hold Breath");
        assert_eq!(m.version, "1.0.3");
        assert_eq!(m.autoloads.len(), 2);
        let boot = &m
            .autoloads
            .iter()
            .find(|a| a.name == "HoldBreathBoot")
            .unwrap();
        assert_eq!(boot.path, "res://HoldBreath/Main.gd");
        assert!(boot.singleton);
        assert_eq!(
            m.extra_sections
                .get("updates")
                .unwrap()
                .get("modworkshop")
                .unwrap(),
            "55938"
        );
    }

    #[test]
    fn missing_mod_section_is_rejected() {
        let err = ModManifest::parse_str("[autoload]\nX=\"res://x.gd\"\n").unwrap_err();
        assert!(matches!(err, CrabbyError::Manifest { .. }));
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let err = ModManifest::parse_str("[mod]\nname=\"x\"\nid=\"x\"\n").unwrap_err();
        assert!(matches!(err, CrabbyError::Manifest { .. }));
    }

    #[test]
    fn bare_autoload_path_is_not_singleton() {
        let src = r#"[mod]
id="x"
name="X"
version="1"

[autoload]
NoStar="res://x.gd"
"#;
        let m = ModManifest::parse_str(src).unwrap();
        assert!(!m.autoloads[0].singleton);
        assert_eq!(m.autoloads[0].path, "res://x.gd");
    }

    #[test]
    fn full_line_comments_are_ignored() {
        let src = r#"; top comment
[mod]
# another comment
id="x"
name="X"
version="1"
"#;
        let m = ModManifest::parse_str(src).unwrap();
        assert_eq!(m.id, "x");
    }

    #[test]
    fn blank_values_are_rejected() {
        let src = "[mod]\nid=\"\"\nname=\"x\"\nversion=\"1\"\n";
        let err = ModManifest::parse_str(src).unwrap_err();
        assert!(matches!(err, CrabbyError::Manifest { .. }));
    }
}
