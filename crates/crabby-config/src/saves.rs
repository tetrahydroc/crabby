//! Save-slot + snapshot management for the launcher's Saves tab.
//!
//! # Layout
//!
//! ```text
//! <user>/                          (RTV's user-data dir; resolved by [`crate::mcm::user_data_dir`])
//! ├── active_slot.txt              key=value plaintext: profile, slot
//! ├── Validator.tres               install-global; not per-slot
//! ├── Preferences.tres             install-global; not per-slot
//! └── saves/
//!     ├── default/                 profile (matches mod_config.cfg's active_profile)
//!     │   ├── default/             slot
//!     │   │   ├── Character.tres
//!     │   │   ├── ...
//!     │   │   └── .snapshots/      per-slot snapshot history (zips)
//!     │   └── ironman-run/         another slot, same profile
//!     └── modded/                  another profile
//!         └── default/             same slot name as above; per-profile uniqueness
//! ```
//!
//! # Profile / slot relationship
//!
//! Slots are scoped to mod profiles: a save built against profile X
//! lives under `saves/X/<slot>/` and is meaningful only with profile
//! X active. Same-named slots can exist across profiles (`default` is
//! per-profile, not global). Restoring across profiles is intentionally
//! not supported in v1, too easy to corrupt a save with the wrong mod
//! set loaded.
//!
//! # active_slot.txt format
//!
//! ```text
//! profile=default
//! slot=default
//! ```
//!
//! Two-key plaintext. Trailing newline tolerated. Anything other than
//! these two keys is ignored. RTV reads this at boot to resolve the
//! save path; the launcher writes it on profile-switch / slot-switch.
//!
//! # Scope (v1)
//!
//! - List slots for a profile (or across all profiles)
//! - Switch active (slot, profile), writes `active_slot.txt`
//! - Create empty slot, delete slot
//! - Snapshot the slot dir to `saves/<profile>/<slot>/.snapshots/<stamp>.zip`
//! - List + restore + delete snapshots (restore stays within owning profile)

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::mcm::user_data_dir;

/// Bare-name slot identifier. Validated by [`is_safe_name`].
pub type SlotName = String;
/// Bare-name profile identifier. Validated by [`is_safe_name`].
pub type ProfileName = String;

/// File that names the active (profile, slot) pair. Plaintext.
pub const ACTIVE_SLOT_FILE: &str = "active_slot.txt";

/// Default slot/profile used when nothing else is set.
pub const DEFAULT_NAME: &str = "default";

/// Directory under `<user>/` that holds all profile dirs.
const SAVES_DIR: &str = "saves";

/// Per-slot subdir for snapshot zips. Excluded from snapshots so we
/// don't recursively pack history into history.
const SNAPSHOTS_DIR: &str = ".snapshots";

/// One discovered slot. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SlotInfo {
    /// Bare slot name (matches the dir name under `<profile>/`).
    pub name: SlotName,
    /// Profile this slot belongs to. Same-named slots across profiles
    /// are independent.
    pub profile: ProfileName,
    /// Absolute path to the slot dir.
    pub path: PathBuf,
    /// Total bytes of slot contents (excluding the `.snapshots/` dir).
    /// `None` when the dir couldn't be walked.
    pub size_bytes: Option<u64>,
    /// True when this slot's `(profile, name)` matches `active_slot.txt`.
    pub active: bool,
}

/// One snapshot zip on disk.
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    /// File name (without the `.zip` extension).
    pub name: String,
    /// Absolute path to the zip.
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Modified time, for sorting newest-first in the UI.
    pub modified: Option<SystemTime>,
}

/// Active (profile, slot) pair from `active_slot.txt`. Defaults
/// applied per-key on missing/unsafe values.
#[derive(Debug, Clone)]
pub struct ActiveTarget {
    /// Active profile name.
    pub profile: ProfileName,
    /// Active slot name (within the profile).
    pub slot: SlotName,
}

impl Default for ActiveTarget {
    fn default() -> Self {
        Self {
            profile: DEFAULT_NAME.into(),
            slot: DEFAULT_NAME.into(),
        }
    }
}

/// Errors returned by the slot/snapshot operations. Stringly because
/// the UI surfaces them inline and typed handling isn't needed above.
#[derive(Debug, thiserror::Error)]
pub enum SavesError {
    /// Couldn't resolve the user-data dir (no APPDATA, no HOME, etc.).
    #[error("Couldn't resolve RTV user data dir for this platform")]
    NoUserDir,
    /// Slot or profile name failed [`is_safe_name`].
    #[error("Invalid name `{0}`, letters/digits/dash/underscore/space only, max 64 chars")]
    InvalidName(String),
    /// Underlying filesystem error with context.
    #[error("{ctx}: {source}")]
    Io {
        /// Operation context (e.g. `"read /path"`) for the message.
        ctx: String,
        /// Underlying io error.
        #[source]
        source: std::io::Error,
    },
    /// Zip read/write error.
    #[error("{ctx}: {source}")]
    Zip {
        /// Operation context for the message.
        ctx: String,
        /// Underlying zip error.
        #[source]
        source: zip::result::ZipError,
    },
    /// Destination slot already contains files that would collide with
    /// an import / cross-profile move. Carries the conflicting filenames
    /// so the UI can surface them.
    #[error("Destination slot `{profile}/{slot}` already has saves: {}", colliding.join(", "))]
    SlotHasCollidingFiles {
        /// Destination profile name.
        profile: ProfileName,
        /// Destination slot name.
        slot: SlotName,
        /// Bare filenames that already exist at the destination.
        colliding: Vec<String>,
    },
}

impl SavesError {
    fn io(ctx: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            ctx: ctx.into(),
            source,
        }
    }
    fn zip(ctx: impl Into<String>, source: zip::result::ZipError) -> Self {
        Self::Zip {
            ctx: ctx.into(),
            source,
        }
    }
}

/// Same validation as RTV's `_rtv_is_safe_slot_name` GDScript helper.
/// Applied to both profile and slot names, same rules.
#[must_use]
pub fn is_safe_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ')
}

/// `<user>/saves/`. Created on demand by [`list_slots`] / [`create_slot`].
#[must_use]
pub fn saves_root() -> Option<PathBuf> {
    user_data_dir().map(|d| d.join(SAVES_DIR))
}

/// `<user>/active_slot.txt`.
#[must_use]
pub fn active_slot_file() -> Option<PathBuf> {
    user_data_dir().map(|d| d.join(ACTIVE_SLOT_FILE))
}

/// Filenames that live at `<user>/` root in both vanilla and crabby and
/// must NOT be moved on import (install-globals, not per-slot state).
const VANILLA_PRESERVED_FILES: &[&str] = &["Validator.tres", "Preferences.tres"];

/// One detected vanilla / loose-root save set. Returned by
/// [`scan_vanilla_root_saves`] when loose `*.tres` saves exist at the
/// user-data dir root (typical for fresh-from-vanilla or VML players
/// who haven't yet imported into a crabby profile).
///
/// Files in the set are the *constituents* of one save state, a single
/// game has multiple `.tres` files (Character, World, Traders, etc.).
/// Treat the set as one logical save, not N independent ones.
#[derive(Debug, Clone)]
pub struct VanillaSaveSet {
    /// Absolute paths to every loose `.tres` at root that isn't an
    /// install-global.
    pub files: Vec<PathBuf>,
    /// Sum of the file sizes in bytes. Useful for the UI summary line.
    pub total_size_bytes: u64,
    /// Newest mtime across the set, for "last played" display. `None`
    /// when no file's metadata could be read (rare).
    pub last_modified: Option<SystemTime>,
}

/// Scan `<user>/` root for vanilla / loose-root save files. Returns
/// `Ok(Some(_))` when at least one non-install-global `.tres` exists,
/// `Ok(None)` when the dir is clean, and `Err` only on truly fatal
/// conditions (no user dir resolvable, root unreadable). Per-file
/// metadata errors are absorbed, a save with one unreadable mtime
/// still appears in the set.
pub fn scan_vanilla_root_saves() -> Result<Option<VanillaSaveSet>, SavesError> {
    let Some(root) = user_data_dir() else {
        return Err(SavesError::NoUserDir);
    };
    if !root.exists() {
        // Vanilla user dir hasn't been created yet, nothing could be
        // there to migrate. Not an error.
        return Ok(None);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut total: u64 = 0;
    let mut newest: Option<SystemTime> = None;
    let preserved: std::collections::HashSet<&str> =
        VANILLA_PRESERVED_FILES.iter().copied().collect();
    let entries =
        fs::read_dir(&root).map_err(|e| SavesError::io(format!("read {}", root.display()), e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        // Top-level files only; subdirs (saves/, MCM/, .crabby/, etc.)
        // are ours or known-other and don't belong in the vanilla set.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".tres") {
            continue;
        }
        if preserved.contains(name) {
            continue;
        }
        // Per-file metadata: tolerate errors, just skip the size/mtime
        // contribution rather than dropping the file from the set.
        if let Ok(meta) = entry.metadata() {
            total = total.saturating_add(meta.len());
            if let Ok(m) = meta.modified() {
                newest = Some(match newest {
                    Some(prev) if prev >= m => prev,
                    _ => m,
                });
            }
        }
        files.push(path);
    }
    if files.is_empty() {
        return Ok(None);
    }
    files.sort();
    Ok(Some(VanillaSaveSet {
        files,
        total_size_bytes: total,
        last_modified: newest,
    }))
}

/// Read the active (profile, slot). Falls back to defaults on missing
/// file, parse error, or unsafe values.
#[must_use]
pub fn active_target() -> ActiveTarget {
    let Some(path) = active_slot_file() else {
        return ActiveTarget::default();
    };
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return ActiveTarget::default(),
    };
    parse_active_slot_file(&raw)
}

/// Parse the two-key plaintext format. Public for round-trip testing.
#[must_use]
pub fn parse_active_slot_file(raw: &str) -> ActiveTarget {
    let mut t = ActiveTarget::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "profile" if is_safe_name(value) => t.profile = value.to_string(),
            "slot" if is_safe_name(value) => t.slot = value.to_string(),
            _ => {}
        }
    }
    t
}

/// Render an [`ActiveTarget`] back to the plaintext format.
#[must_use]
pub fn render_active_slot_file(t: &ActiveTarget) -> String {
    format!("profile={}\nslot={}\n", t.profile, t.slot)
}

/// Write the active (profile, slot). Both names go through
/// [`is_safe_name`]. Creates the user dir if needed.
pub fn set_active_target(profile: &str, slot: &str) -> Result<(), SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let dir = user_data_dir().ok_or(SavesError::NoUserDir)?;
    fs::create_dir_all(&dir).map_err(|e| SavesError::io(format!("create {}", dir.display()), e))?;
    let path = dir.join(ACTIVE_SLOT_FILE);
    let body = render_active_slot_file(&ActiveTarget {
        profile: profile.into(),
        slot: slot.into(),
    });
    fs::write(&path, body).map_err(|e| SavesError::io(format!("write {}", path.display()), e))?;
    Ok(())
}

/// `<user>/saves/<profile>/`.
#[must_use]
pub fn profile_dir(profile: &str) -> Option<PathBuf> {
    saves_root().map(|r| r.join(profile))
}

/// `<user>/saves/<profile>/<slot>/`.
#[must_use]
pub fn slot_dir(profile: &str, slot: &str) -> Option<PathBuf> {
    profile_dir(profile).map(|p| p.join(slot))
}

/// `<user>/saves/<profile>/<slot>/.snapshots/`.
#[must_use]
pub fn snapshots_dir(profile: &str, slot: &str) -> Option<PathBuf> {
    slot_dir(profile, slot).map(|s| s.join(SNAPSHOTS_DIR))
}

/// List slots under one profile. Empty vec if the profile dir is
/// missing.
pub fn list_slots(profile: &str) -> Result<Vec<SlotInfo>, SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    let Some(dir) = profile_dir(profile) else {
        return Err(SavesError::NoUserDir);
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let active = active_target();
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&dir).map_err(|e| SavesError::io(format!("read {}", dir.display()), e))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !is_safe_name(&name) {
            continue;
        }
        let size_bytes = dir_size_excluding(&path, &[SNAPSHOTS_DIR]).ok();
        let active = active.profile == profile && active.slot == name;
        out.push(SlotInfo {
            name,
            profile: profile.to_string(),
            path,
            size_bytes,
            active,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// List slots across every profile dir under `saves/`. Returned vec
/// is sorted by `(profile, slot)`.
pub fn list_all_slots() -> Result<Vec<SlotInfo>, SavesError> {
    let Some(root) = saves_root() else {
        return Err(SavesError::NoUserDir);
    };
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&root).map_err(|e| SavesError::io(format!("read {}", root.display()), e))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let profile = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !is_safe_name(&profile) {
            continue;
        }
        out.extend(list_slots(&profile)?);
    }
    out.sort_by(|a, b| {
        (a.profile.as_str(), a.name.as_str()).cmp(&(b.profile.as_str(), b.name.as_str()))
    });
    Ok(out)
}

/// List discovered profile names (i.e. dirs under `saves/`). Useful
/// for the "show all profiles" toggle in the UI.
pub fn list_profiles() -> Result<Vec<ProfileName>, SavesError> {
    let Some(root) = saves_root() else {
        return Err(SavesError::NoUserDir);
    };
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&root).map_err(|e| SavesError::io(format!("read {}", root.display()), e))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            if is_safe_name(n) {
                out.push(n.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Create an empty slot dir for a profile. Idempotent.
pub fn create_slot(profile: &str, slot: &str) -> Result<PathBuf, SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let path = slot_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    fs::create_dir_all(&path)
        .map_err(|e| SavesError::io(format!("create {}", path.display()), e))?;
    Ok(path)
}

/// Delete a slot dir and all its contents (including snapshots).
/// Refuses to delete the active slot.
pub fn delete_slot(profile: &str, slot: &str) -> Result<(), SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let active = active_target();
    if active.profile == profile && active.slot == slot {
        return Err(SavesError::Io {
            ctx: format!("refusing to delete active slot `{profile}/{slot}`"),
            source: std::io::Error::new(
                std::io::ErrorKind::Other,
                "switch to a different slot first",
            ),
        });
    }
    let path = slot_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    if !path.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&path)
        .map_err(|e| SavesError::io(format!("remove {}", path.display()), e))?;
    Ok(())
}

/// Per-call report from [`import_vanilla_to_slot`] and
/// [`move_slot_between_profiles`]. Mirrors the file-level outcome so
/// the UI can render a "moved 5 file(s)" line and the launcher can log
/// what happened.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    /// Bare filenames moved (no path).
    pub moved: Vec<String>,
}

/// Import a [`VanillaSaveSet`]'s files into the given (profile, slot).
/// Slot is created if missing. The collision policy is **refuse**: if
/// the slot already contains any file whose name matches a vanilla
/// file to be moved, return [`SavesError::SlotHasCollidingFiles`] without
/// touching anything. Source files are deleted from `<user>/` root on
/// success.
///
/// Files outside `set.files` (e.g. random `.txt` the user dropped at
/// root) are not touched. Callers that want to move arbitrary loose
/// files should curate the [`VanillaSaveSet`] before passing it in.
pub fn import_vanilla_to_slot(
    set: &VanillaSaveSet,
    profile: &str,
    slot: &str,
) -> Result<ImportReport, SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let dst_dir = slot_dir(profile, slot).ok_or(SavesError::NoUserDir)?;

    // Pre-flight: list source filenames + check collisions against
    // existing dst contents BEFORE touching anything. The destination
    // might not exist yet (fresh slot), in which case there can be no
    // collisions.
    let src_names: Vec<String> = set
        .files
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    if dst_dir.exists() {
        let existing: std::collections::HashSet<String> = fs::read_dir(&dst_dir)
            .map_err(|e| SavesError::io(format!("read {}", dst_dir.display()), e))?
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();
        let colliding: Vec<String> = src_names
            .iter()
            .filter(|n| existing.contains(*n))
            .cloned()
            .collect();
        if !colliding.is_empty() {
            return Err(SavesError::SlotHasCollidingFiles {
                profile: profile.into(),
                slot: slot.into(),
                colliding,
            });
        }
    } else {
        fs::create_dir_all(&dst_dir)
            .map_err(|e| SavesError::io(format!("create {}", dst_dir.display()), e))?;
    }

    // Move each file. We use copy-then-delete instead of rename because
    // the user-data dir might span filesystems on some Linux setups and
    // rename across mounts fails with EXDEV.
    let mut moved = Vec::with_capacity(set.files.len());
    for src in &set.files {
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let dst = dst_dir.join(name);
        fs::copy(src, &dst).map_err(|e| {
            SavesError::io(format!("copy {} -> {}", src.display(), dst.display()), e)
        })?;
        fs::remove_file(src).map_err(|e| SavesError::io(format!("remove {}", src.display()), e))?;
        moved.push(name.to_string());
    }
    Ok(ImportReport { moved })
}

/// Move the entire slot directory `(src_profile, src_slot)` to
/// `(dst_profile, dst_slot)`, every loose save file AND the
/// `.snapshots/` subdir come along. After a successful move the empty
/// source slot dir is removed.
///
/// Same collision policy as [`import_vanilla_to_slot`]: refuse if any
/// destination filename already exists (including a pre-existing
/// `.snapshots/` subdir at the destination). No partial application,
/// either everything moves or nothing does.
///
/// Refuses to move from / to the active slot to avoid races with a
/// running game session.
pub fn move_slot_between_profiles(
    src_profile: &str,
    src_slot: &str,
    dst_profile: &str,
    dst_slot: &str,
) -> Result<ImportReport, SavesError> {
    for n in [src_profile, src_slot, dst_profile, dst_slot] {
        if !is_safe_name(n) {
            return Err(SavesError::InvalidName(n.into()));
        }
    }
    if src_profile == dst_profile && src_slot == dst_slot {
        return Ok(ImportReport::default());
    }
    let active = active_target();
    for (p, s, label) in [
        (src_profile, src_slot, "source"),
        (dst_profile, dst_slot, "destination"),
    ] {
        if active.profile == p && active.slot == s {
            return Err(SavesError::Io {
                ctx: format!("refusing move: {label} `{p}/{s}` is the active slot"),
                source: std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "switch to a different slot first",
                ),
            });
        }
    }

    let src_dir = slot_dir(src_profile, src_slot).ok_or(SavesError::NoUserDir)?;
    let dst_dir = slot_dir(dst_profile, dst_slot).ok_or(SavesError::NoUserDir)?;
    if !src_dir.exists() {
        return Ok(ImportReport::default());
    }

    // Collect every entry in the source dir, both files and the
    // `.snapshots/` subdir. Move semantics is "rename the slot dir",
    // so everything inside comes with it.
    let mut src_entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&src_dir)
        .map_err(|e| SavesError::io(format!("read {}", src_dir.display()), e))?
        .flatten()
    {
        src_entries.push(entry.path());
    }
    if src_entries.is_empty() {
        // Nothing to move; remove the empty source dir to keep the
        // post-move shape consistent ("move N -> M" leaves N gone).
        let _ = fs::remove_dir(&src_dir);
        return Ok(ImportReport::default());
    }

    // Collision check, refuse if any destination name (file or
    // subdir) already exists. .snapshots/ collisions are real:
    // merging two snapshot histories isn't well-defined, so reject
    // rather than guess.
    let src_names: Vec<String> = src_entries
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    if dst_dir.exists() {
        let existing: std::collections::HashSet<String> = fs::read_dir(&dst_dir)
            .map_err(|e| SavesError::io(format!("read {}", dst_dir.display()), e))?
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();
        let colliding: Vec<String> = src_names
            .iter()
            .filter(|n| existing.contains(*n))
            .cloned()
            .collect();
        if !colliding.is_empty() {
            return Err(SavesError::SlotHasCollidingFiles {
                profile: dst_profile.into(),
                slot: dst_slot.into(),
                colliding,
            });
        }
    } else {
        fs::create_dir_all(&dst_dir)
            .map_err(|e| SavesError::io(format!("create {}", dst_dir.display()), e))?;
    }

    // Move each entry. Files are copy-then-delete (cross-fs safe).
    // The `.snapshots/` subdir is recursively copied then removed,
    // same reason: fs::rename refuses cross-device on most platforms.
    let mut moved = Vec::with_capacity(src_entries.len());
    for src in &src_entries {
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let dst = dst_dir.join(name);
        if src.is_dir() {
            copy_dir_recursive(src, &dst)?;
            fs::remove_dir_all(src)
                .map_err(|e| SavesError::io(format!("remove dir {}", src.display()), e))?;
        } else {
            fs::copy(src, &dst).map_err(|e| {
                SavesError::io(format!("copy {} -> {}", src.display(), dst.display()), e)
            })?;
            fs::remove_file(src)
                .map_err(|e| SavesError::io(format!("remove {}", src.display()), e))?;
        }
        moved.push(name.to_string());
    }
    // Source dir should now be empty, remove it so the slot fully
    // disappears from the source profile. Best-effort; if some
    // unexpected file landed in there mid-move (shouldn't happen
    // since moves involving the active slot are refused), leave it.
    let _ = fs::remove_dir(&src_dir);
    Ok(ImportReport { moved })
}

/// Recursive directory copy. Used by the slot-move path to bring
/// `.snapshots/` along with the slot. `dst` is created if missing.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), SavesError> {
    fs::create_dir_all(dst).map_err(|e| SavesError::io(format!("create {}", dst.display()), e))?;
    for entry in fs::read_dir(src)
        .map_err(|e| SavesError::io(format!("read {}", src.display()), e))?
        .flatten()
    {
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        let dst_path = dst.join(name);
        if path.is_dir() {
            copy_dir_recursive(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path).map_err(|e| {
                SavesError::io(
                    format!("copy {} -> {}", path.display(), dst_path.display()),
                    e,
                )
            })?;
        }
    }
    Ok(())
}

/// List snapshots for one (profile, slot). Sorted newest-first.
pub fn list_snapshots(profile: &str, slot: &str) -> Result<Vec<SnapshotInfo>, SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let dir = snapshots_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&dir).map_err(|e| SavesError::io(format!("read {}", dir.display()), e))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("zip") {
            continue;
        }
        let name = match path.file_stem().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(SnapshotInfo {
            name,
            path,
            size_bytes: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

/// Snapshot the slot dir into its own `.snapshots/<file_name>.zip`.
pub fn snapshot_slot(profile: &str, slot: &str, file_name: &str) -> Result<PathBuf, SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let safe_name = sanitize_for_filename(file_name);
    let snap_dir = snapshots_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    fs::create_dir_all(&snap_dir)
        .map_err(|e| SavesError::io(format!("create {}", snap_dir.display()), e))?;
    let zip_path = snap_dir.join(format!("{safe_name}.zip"));
    let tmp_path = snap_dir.join(format!(".{safe_name}.zip.tmp"));
    let sd = slot_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    if !sd.exists() {
        return Err(SavesError::Io {
            ctx: format!("snapshot {profile}/{slot}"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "slot dir missing"),
        });
    }

    {
        let f = fs::File::create(&tmp_path)
            .map_err(|e| SavesError::io(format!("create {}", tmp_path.display()), e))?;
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        write_dir_to_zip(&mut zw, &sd, &sd, &opts)
            .map_err(|e| SavesError::zip(format!("write zip {}", tmp_path.display()), e))?;
        zw.finish()
            .map_err(|e| SavesError::zip(format!("finish zip {}", tmp_path.display()), e))?;
    }

    if let Err(e) = fs::rename(&tmp_path, &zip_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(SavesError::io(
            format!("rename {} -> {}", tmp_path.display(), zip_path.display()),
            e,
        ));
    }
    Ok(zip_path)
}

/// Restore a snapshot into its owning (profile, slot). The caller
/// supplies both, UI must not let the user restore across profiles.
pub fn restore_snapshot(profile: &str, slot: &str, snapshot_path: &Path) -> Result<(), SavesError> {
    if !is_safe_name(profile) {
        return Err(SavesError::InvalidName(profile.into()));
    }
    if !is_safe_name(slot) {
        return Err(SavesError::InvalidName(slot.into()));
    }
    let sd = slot_dir(profile, slot).ok_or(SavesError::NoUserDir)?;
    fs::create_dir_all(&sd).map_err(|e| SavesError::io(format!("create {}", sd.display()), e))?;

    // Wipe existing slot contents except `.snapshots/`.
    for entry in
        fs::read_dir(&sd).map_err(|e| SavesError::io(format!("read {}", sd.display()), e))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if p.file_name().and_then(|n| n.to_str()) == Some(SNAPSHOTS_DIR) {
            continue;
        }
        if p.is_dir() {
            fs::remove_dir_all(&p)
                .map_err(|e| SavesError::io(format!("remove {}", p.display()), e))?;
        } else {
            fs::remove_file(&p)
                .map_err(|e| SavesError::io(format!("remove {}", p.display()), e))?;
        }
    }

    let f = fs::File::open(snapshot_path)
        .map_err(|e| SavesError::io(format!("open {}", snapshot_path.display()), e))?;
    let mut zr = zip::ZipArchive::new(f)
        .map_err(|e| SavesError::zip(format!("read zip {}", snapshot_path.display()), e))?;
    for i in 0..zr.len() {
        let mut entry = zr
            .by_index(i)
            .map_err(|e| SavesError::zip(format!("zip entry {i}"), e))?;
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let dest = sd.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest)
                .map_err(|e| SavesError::io(format!("mkdir {}", dest.display()), e))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SavesError::io(format!("mkdir {}", parent.display()), e))?;
        }
        let mut out = fs::File::create(&dest)
            .map_err(|e| SavesError::io(format!("create {}", dest.display()), e))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| SavesError::io(format!("write {}", dest.display()), e))?;
    }
    Ok(())
}

/// Delete one snapshot zip.
pub fn delete_snapshot(snapshot_path: &Path) -> Result<(), SavesError> {
    fs::remove_file(snapshot_path)
        .map_err(|e| SavesError::io(format!("remove {}", snapshot_path.display()), e))?;
    Ok(())
}

/// Recursively walk `dir`, summing file sizes. Skips top-level dirs
/// matching any name in `skip_dirs` (matched by file name, not path).
fn dir_size_excluding(dir: &Path, skip_dirs: &[&str]) -> Result<u64, std::io::Error> {
    let mut total = 0_u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if skip_dirs.contains(&name) {
                    continue;
                }
            }
            total += dir_size_excluding(&p, skip_dirs)?;
        } else {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

/// Recursively pack `dir` into the open zip writer. Paths inside the
/// zip are made relative to `root` so unzipping into a different
/// destination dir works. Skips the per-slot snapshots subdir.
fn write_dir_to_zip<W: Write + std::io::Seek>(
    zw: &mut zip::ZipWriter<W>,
    dir: &Path,
    root: &Path,
    opts: &zip::write::FileOptions<()>,
) -> Result<(), zip::result::ZipError> {
    for entry in fs::read_dir(dir).map_err(zip::result::ZipError::Io)? {
        let entry = entry.map_err(zip::result::ZipError::Io)?;
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some(SNAPSHOTS_DIR) {
                continue;
            }
            write_dir_to_zip(zw, &p, root, opts)?;
        } else {
            let rel = p
                .strip_prefix(root)
                .map_err(|_| zip::result::ZipError::FileNotFound)?;
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            zw.start_file(rel_str, *opts)?;
            let mut f = fs::File::open(&p).map_err(zip::result::ZipError::Io)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(zip::result::ZipError::Io)?;
            zw.write_all(&buf).map_err(zip::result::ZipError::Io)?;
        }
    }
    Ok(())
}

/// Generate a default snapshot name from the current local time.
/// Format: `auto-YYYYMMDD-HHMMSS`.
#[must_use]
pub fn default_snapshot_name() -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = now.as_secs() as i64;
    let (y, mo, d, h, mi, s) = utc_breakdown(secs);
    format!("auto-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Coerce a user-supplied snapshot name into something safe for the
/// filesystem.
#[must_use]
pub fn sanitize_for_filename(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out = out.trim().trim_start_matches('.').to_string();
    if out.is_empty() {
        return default_snapshot_name();
    }
    if out.len() > 96 {
        out.truncate(96);
    }
    out
}

/// Tiny gregorian breakdown for filename stamps. Civil-from-days
/// algorithm by Howard Hinnant (public domain).
fn utc_breakdown(unix_secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let secs_per_day = 86_400_i64;
    let days = unix_secs.div_euclid(secs_per_day);
    let secs_today = unix_secs.rem_euclid(secs_per_day) as u32;
    let h = secs_today / 3600;
    let mi = (secs_today / 60) % 60;
    let s = secs_today % 60;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);
    (y as i32, m, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert!(is_safe_name("default"));
        assert!(is_safe_name("ironman-run"));
        assert!(is_safe_name("My Save 1"));
    }

    #[test]
    fn unsafe_names() {
        assert!(!is_safe_name(""));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name("a/b"));
        assert!(!is_safe_name("a\\b"));
        assert!(!is_safe_name("a..b"));
    }

    #[test]
    fn sanitize_drops_unsafe_chars() {
        assert_eq!(sanitize_for_filename("hello world"), "hello world");
        assert_eq!(sanitize_for_filename("with/slash"), "with_slash");
    }

    #[test]
    fn sanitize_falls_back_when_empty() {
        assert!(sanitize_for_filename("").starts_with("auto-"));
        assert!(sanitize_for_filename("   ").starts_with("auto-"));
    }

    #[test]
    fn default_name_format() {
        let n = default_snapshot_name();
        assert_eq!(n.len(), 20, "{n}");
        assert!(n.starts_with("auto-"));
    }

    #[test]
    fn utc_breakdown_smoke() {
        assert_eq!(utc_breakdown(1_777_892_096), (2026, 5, 4, 10, 54, 56));
        assert_eq!(utc_breakdown(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn parses_active_slot_file() {
        let raw = "profile=modded\nslot=ironman\n";
        let t = parse_active_slot_file(raw);
        assert_eq!(t.profile, "modded");
        assert_eq!(t.slot, "ironman");
    }

    #[test]
    fn parse_tolerates_trailing_newline_and_spaces() {
        let raw = "  profile = default \n  slot = My Save 1  \n";
        let t = parse_active_slot_file(raw);
        assert_eq!(t.profile, "default");
        assert_eq!(t.slot, "My Save 1");
    }

    #[test]
    fn parse_falls_back_per_key() {
        // Bad slot, good profile, only slot defaults.
        let raw = "profile=ok\nslot=../escape\n";
        let t = parse_active_slot_file(raw);
        assert_eq!(t.profile, "ok");
        assert_eq!(t.slot, DEFAULT_NAME);
    }

    #[test]
    fn collision_error_renders_filenames() {
        let err = SavesError::SlotHasCollidingFiles {
            profile: "default".into(),
            slot: "default".into(),
            colliding: vec!["Character.tres".into(), "World.tres".into()],
        };
        let msg = err.to_string();
        assert!(msg.contains("default/default"), "{msg}");
        assert!(msg.contains("Character.tres"), "{msg}");
        assert!(msg.contains("World.tres"), "{msg}");
    }

    #[test]
    fn parse_ignores_unknown_keys_and_comments() {
        let raw = "# hi\nprofile=p1\nfoo=bar\nslot=s1\n";
        let t = parse_active_slot_file(raw);
        assert_eq!(t.profile, "p1");
        assert_eq!(t.slot, "s1");
    }

    #[test]
    fn render_round_trips() {
        let t = ActiveTarget {
            profile: "p1".into(),
            slot: "s1".into(),
        };
        let raw = render_active_slot_file(&t);
        let parsed = parse_active_slot_file(&raw);
        assert_eq!(parsed.profile, t.profile);
        assert_eq!(parsed.slot, t.slot);
    }
}
