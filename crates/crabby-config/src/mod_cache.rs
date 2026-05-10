//! Pre-rewritten mod-archive cache.
//!
//! Mods shipped as `.vmz` / `.zip` archives can contain GDScript files
//! that use `const X = preload("foo.tres")`. Under crabby's PCK-baked
//! environment, those const-bound preloaded Resources behave like value
//! snapshots for property reads, so mutations made elsewhere (e.g. via
//! a Config.gd writing to the same Resource) are invisible to the const
//! reader. The fix is mechanical: rewrite `const NAME [:= | =]
//! preload("...tres")` → `var NAME = preload(...)` in every `.gd` file
//! inside the archive.
//!
//! Historically the rewrite ran in the runtime shim
//! (`shim/crabby_shim.gd::_vmz_to_zip_cache`) on every launch. That
//! cost ~10-50ms per enabled archive and put a noticeable startup tax
//! on multi-mod installs. Doing it once per install/refresh in Rust
//! (this module) and persisting the result means the runtime shim
//! mounts the cached file directly with no rewrite at boot.
//!
//! # Cache layout
//!
//! `<game-dir>/.crabby/mod_cache/<id>.<src_mtime>.zip`, file-stem
//! includes the source archive's mtime so:
//!
//! - Refreshing after the source vmz changes produces a new file under
//!   a different stem; the old one becomes orphan and gets GC'd by the
//!   next `rebuild_for_enabled` pass.
//! - The shim can match by exact filename and skip rebuilding when the
//!   cache is current.
//!
//! Folder mods are NOT cached here, their dev workflow (edit folder
//! -> re-launch picks up changes) is incompatible with persistent
//! caching. The runtime shim's `_folder_to_zip_cache` continues to
//! handle them per-launch.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crabby_error::{CrabbyError, Result};
use crabby_manifest::ModSource;

use crate::mod_index::ModIndex;

/// Subdirectory under `<game-dir>/.crabby/` where pre-rewritten archives
/// live. Created on first cache write; safe to wipe at any time (next
/// `rebuild_for_enabled` will repopulate).
pub const MOD_CACHE_REL_DIR: &str = ".crabby/mod_cache";

/// Compute the cache filename for `mod_id` at `src_mtime`. Includes the
/// mtime so stale entries are visibly distinct from fresh ones and get
/// GC'd in `rebuild_for_enabled`.
#[must_use]
pub fn cache_filename(mod_id: &str, src_mtime: i64) -> String {
    // mod_id is allowed to contain slashes / colons / etc. for general
    // ids; sanitize to be filename-safe. Same slug rule the shim uses
    // for its in-memory zip cache so paths stay portable.
    let safe = mod_id
        .replace(['/', '\\', ':'], "_");
    format!("{safe}.{src_mtime}.zip")
}

/// Cache file path for `mod_id` at `src_mtime` inside `game_dir`.
#[must_use]
pub fn cache_path_for(game_dir: &Path, mod_id: &str, src_mtime: i64) -> PathBuf {
    game_dir
        .join(MOD_CACHE_REL_DIR)
        .join(cache_filename(mod_id, src_mtime))
}

/// Rebuild the cache for every vmz/zip-source mod listed in `index`.
///
/// For each:
/// - Compute the canonical cache path (`<id>.<src_mtime>.zip`).
/// - If the file already exists at that path, skip, already current.
/// - Otherwise read the source archive, transform `.gd` entries via
///   [`rewrite_const_resource_preloads`], write to the cache path.
///
/// After processing all entries, garbage-collect any cache files whose
/// `<id>` matches an entry but whose mtime stem doesn't, those are
/// stale outputs from older revisions of the same mod.
///
/// Returns the number of cache entries built. Folder mods are skipped
/// (they're handled by the runtime shim's per-launch zip path).
pub fn rebuild_for_enabled(game_dir: &Path, index: &ModIndex) -> Result<usize> {
    let cache_dir = game_dir.join(MOD_CACHE_REL_DIR);
    fs::create_dir_all(&cache_dir)
        .map_err(|s| CrabbyError::io_at(cache_dir.clone(), s))?;

    let mut built = 0usize;
    // Track every filename touched (built or already-current) so the
    // GC pass below can identify orphans.
    let mut current: HashSet<String> = HashSet::new();
    for (id, entry) in &index.entries {
        // Folder mods don't fit a persistent-cache model; skip.
        if entry.source.eq_ignore_ascii_case("folder") {
            continue;
        }
        if !entry.path.is_file() {
            // Source archive missing, skip without erroring; the
            // runtime shim will surface this when it tries to mount.
            continue;
        }
        let filename = cache_filename(id, entry.mtime);
        let dst = cache_dir.join(&filename);
        current.insert(filename);
        if dst.is_file() {
            // Already cached at this mtime, nothing to do.
            continue;
        }
        rewrite_archive(&entry.path, &dst)?;
        built += 1;
    }

    // GC: drop files in the cache dir that aren't current. Keeps the
    // dir size bounded as users update mods.
    if let Ok(read_dir) = fs::read_dir(&cache_dir) {
        for entry in read_dir.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else { continue };
            if !current.contains(name_str) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    Ok(built)
}

/// Read `src` archive, rewrite `.gd` entries, write to `dst`.
///
/// Pure function over file paths, no global state, fully testable
/// with temp dirs. The transform is applied to `.gd` entries only;
/// every other entry is copied through verbatim.
pub fn rewrite_archive(src: &Path, dst: &Path) -> Result<()> {
    let f = fs::File::open(src)
        .map_err(|s| CrabbyError::io_at(src.to_path_buf(), s))?;
    let mut zip_in = zip::ZipArchive::new(f).map_err(|s| CrabbyError::Config {
        context: format!("opening {} as zip", src.display()),
        source: Box::new(s),
    })?;

    // Atomic write: tmp file + rename so a crash mid-write doesn't
    // leave a half-built cache that fools subsequent runs.
    let tmp = dst.with_extension("zip.tmp");
    if let Some(parent) = tmp.parent() {
        fs::create_dir_all(parent)
            .map_err(|s| CrabbyError::io_at(parent.to_path_buf(), s))?;
    }
    let f_out = fs::File::create(&tmp)
        .map_err(|s| CrabbyError::io_at(tmp.clone(), s))?;
    let mut zip_out = zip::ZipWriter::new(f_out);
    // Match the shim's zip output: `Stored` (no compression) so mount
    // time isn't paying decompression cost. Vmz archives in the wild
    // are mixed; normalized to stored for runtime perf.
    let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);

    for i in 0..zip_in.len() {
        let mut entry = zip_in.by_index(i).map_err(|s| CrabbyError::Config {
            context: format!("reading entry {i} from {}", src.display()),
            source: Box::new(s),
        })?;
        let entry_name = entry.name().to_owned();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).map_err(|s| CrabbyError::io_at(src.to_path_buf(), s))?;
        if entry_name.to_ascii_lowercase().ends_with(".gd") {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let rewritten = rewrite_const_resource_preloads(text);
                if rewritten.as_str() != text {
                    bytes = rewritten.into_bytes();
                }
            }
            // Non-UTF-8 .gd bytes are weird (Godot writes UTF-8), pass
            // through unchanged rather than corrupting the data.
        }
        zip_out
            .start_file(&entry_name, opts)
            .map_err(|s| CrabbyError::Config {
                context: format!("writing entry {entry_name} into {}", tmp.display()),
                source: Box::new(s),
            })?;
        zip_out
            .write_all(&bytes)
            .map_err(|s| CrabbyError::io_at(tmp.clone(), s))?;
    }
    zip_out.finish().map_err(|s| CrabbyError::Config {
        context: format!("finalizing {}", tmp.display()),
        source: Box::new(s),
    })?;

    // Remove existing target before rename, Windows refuses to
    // overwrite via std::fs::rename.
    if dst.is_file() {
        let _ = fs::remove_file(dst);
    }
    fs::rename(&tmp, dst).map_err(|s| CrabbyError::io_at(dst.to_path_buf(), s))?;
    Ok(())
}

/// Rewrite `const NAME [:= | =] preload("...tres")` ->
/// `var NAME = preload(...)` so the binding is a live reference, not a
/// value snapshot. Only touches `.tres` preloads (Resource instances);
/// `.gd` / `.tscn` preloads stay unchanged (Script / PackedScene
/// behave correctly as const).
///
/// Conservative: only rewrites top-level statements (no leading
/// whitespace). Mod scripts using indented const declarations inside
/// functions don't trigger this; that's fine because function-local
/// consts can't be the source of cross-frame staleness anyway.
///
/// Mirrors the shim's `_rewrite_const_resource_preloads` 1:1, the
/// shim's GDScript copy stays as the runtime fallback when the cache
/// is missing (e.g. mod added after the launcher's last refresh).
#[must_use]
pub fn rewrite_const_resource_preloads(text: &str) -> String {
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed_line_end = line.trim_end_matches('\n');
        if let Some(rewritten) = rewrite_one_line(trimmed_line_end) {
            out.push_str(&rewritten);
            if line.ends_with('\n') {
                out.push('\n');
            }
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    if changed { out } else { text.to_string() }
}

/// Try to rewrite a single line. Returns `Some(new_line)` on a match,
/// `None` otherwise.
fn rewrite_one_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("const ")?;
    if !line.contains("preload(") {
        return None;
    }
    if !line.contains(".tres\"") {
        return None;
    }
    Some(format!("var {}", rest.replace(":=", "=")))
}

/// True when `path` is the canonical cache file for `mod_id` at the
/// `src_mtime` snapshotted into `index_entry`. The runtime shim uses
/// this to confirm the index entry's `cache_path` is still valid before
/// mounting it.
#[must_use]
pub fn is_cache_current(game_dir: &Path, mod_id: &str, src_path: &Path) -> bool {
    let Some(mtime) = mtime_of(src_path) else {
        return false;
    };
    let cache = cache_path_for(game_dir, mod_id, mtime);
    cache.is_file()
}

/// Best-effort mtime read in unix seconds. None = couldn't read; the
/// caller treats that as cache-miss.
fn mtime_of(path: &Path) -> Option<i64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Returns the `ModSource` matching the index entry's `source` string.
/// Used by callers that want to filter on source kind.
#[must_use]
pub fn parse_source(label: &str) -> Option<ModSource> {
    match label {
        "vmz" => Some(ModSource::Vmz),
        "zip" => Some(ModSource::Zip),
        "folder" => Some(ModSource::Folder),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_const_to_var_for_tres_preload() {
        let src = r#"extends Node
const Settings := preload("res://settings.tres")
const Other := preload("res://script.gd")
var foo: int = 0
"#;
        let out = rewrite_const_resource_preloads(src);
        assert!(out.contains("var Settings = preload(\"res://settings.tres\")"), "{out}");
        // Script preload stays unchanged.
        assert!(out.contains("const Other := preload(\"res://script.gd\")"), "{out}");
        // Other lines untouched.
        assert!(out.contains("var foo: int = 0"));
    }

    #[test]
    fn pass_through_when_no_const_preload_tres() {
        let src = "extends Node\nvar x = 1\n";
        assert_eq!(rewrite_const_resource_preloads(src), src);
    }

    #[test]
    fn cache_filename_embeds_mtime() {
        assert_eq!(cache_filename("foo", 1714850000), "foo.1714850000.zip");
    }

    #[test]
    fn cache_filename_sanitizes_slashes() {
        assert_eq!(cache_filename("ns/id:weird", 1), "ns_id_weird.1.zip");
    }
}
