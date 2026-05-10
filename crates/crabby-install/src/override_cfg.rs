//! `override.cfg` generation.
//!
//! Godot reads `override.cfg` at engine startup, before any script runs.
//! Autoloads declared under `[autoload_prepend]` run before the game's own
//! autoloads, which is how Lib comes up first.
//!
//! # Section preservation
//!
//! Users may have non-autoload sections in their `override.cfg` (display
//! resolution tweaks, input remappings, etc). Crabby must preserve those
//! verbatim; only the `[autoload_prepend]` section (and the placeholder
//! `[autoload]` anchor) is owned here. Unrecognized sections pass through
//! untouched.
//!
//! # Autoload ordering
//!
//! `[autoload_prepend]` is reverse-insertion: the *last* entry listed runs
//! *first*. Lib is the only emitted entry; mod autoloads come from the
//! game's `[autoload]` section (set up via project.binary's autoload
//! list), so they fire after Lib's `_ready`.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crabby_error::{CrabbyError, Result};

use crate::artifacts::{LIB_AUTOLOAD_NAME, LIB_PCK_PATH};

/// Render the full `override.cfg` text for an install.
///
/// Output shape mirrors vostok-mod-loader's override.cfg **exactly**
/// because Godot's engine-startup config parser is more restrictive than
/// the runtime `ConfigFile` class. Leading comments, stray blank lines,
/// and non-ASCII bytes have all been observed to make Godot silently
/// ignore the whole file, producing autoloads that never fire. Vostok's
/// minimal layout is known-working against the target game build, so the
/// output matches it byte-for-byte structurally:
///
/// ```text
/// [autoload_prepend]
/// Lib="*res://Lib.gd"
///
/// [autoload]
/// <preserved sections>
/// ```
///
/// `preserved` is the verbatim non-`[autoload_prepend]` / non-`[autoload]`
/// portion of any pre-existing `override.cfg`, produced by
/// [`extract_preserved_sections`]. It's emitted after the crabby block so
/// user config survives reinstalls.
///
/// Ownership detection is handled out-of-band via the install manifest;
/// crabby keeps no in-file marker so the output stays minimal.
#[must_use]
pub fn render(preserved: &str) -> String {
    let mut out = String::new();
    out.push_str("[autoload_prepend]\n");
    // Lib is the sole entry. Its `_ready` (in shim/lib/boot.gd, appended
    // onto the Lib class via LIB_FRAGMENTS concat) is the first script
    // to run, since it sets `Engine.set_meta("RTVModLib", self)` for
    // back-compat, mounts user mod packs from .crabby/mod_config.cfg,
    // and `call_deferred("_emit_frameworks_ready")` so mods can connect
    // to the signal in their own `_ready` without missing the emission.
    //
    // `Lib` lives at `res://Lib.gd` inside RTV.pck (emitted as a PCK
    // addition during bake, see crabby-bake::bake_pck).
    let _ = writeln!(out, "{LIB_AUTOLOAD_NAME}=\"*{LIB_PCK_PATH}\"");
    // The empty [autoload] section header terminates [autoload_prepend]
    // and signals "no more autoloads to add". Godot silently ignores
    // [autoload_prepend] when this anchor is missing. Do NOT add a
    // trailing blank line here; vostok's working output ends the section
    // at the header newline and the format matches that.
    out.push_str("\n[autoload]\n");
    if !preserved.is_empty() {
        out.push_str(preserved);
        if !preserved.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Split an INI-style `override.cfg` text into (`autoload_prepend` body,
/// other sections verbatim).
///
/// The `[autoload_prepend]` body is discarded, since it is rewritten
/// fresh each install. The placeholder `[autoload]` section (always
/// emitted empty) is also discarded since it is re-emitted. Everything
/// else is preserved verbatim so user-authored sections like `[display]`
/// survive reinstalls.
///
/// A leading UTF-8 BOM (`\u{FEFF}`) is stripped, since some Windows
/// editors save text files with one, and feeding it back could place
/// non-ASCII bytes at the top of the file, which Godot's engine-startup
/// parser has been observed to mishandle.
#[must_use]
pub fn extract_preserved_sections(text: &str) -> String {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = String::new();
    // "Drop this section entirely": true while inside an owned section
    // ([autoload_prepend] or the placeholder [autoload]).
    let mut in_owned_section = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if is_section_header(trimmed) {
            // Header looks like `[name]`, so normalize to lowercase for
            // the ownership check.
            let lower = trimmed.to_ascii_lowercase();
            in_owned_section = lower == "[autoload_prepend]" || lower == "[autoload]";
            if !in_owned_section {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        if !in_owned_section {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_start().to_string()
}

/// Whether `trimmed` is a well-formed `[name]` section header.
///
/// Requires the line to start with `[` and end with `]` with at least
/// one non-whitespace character inside. Lines like `[` alone, `]`, or
/// empty brackets `[]` aren't valid section starts.
fn is_section_header(trimmed: &str) -> bool {
    let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    !inner.trim().is_empty()
}

/// Write `text` atomically to `path` via a `.tmp` file + rename.
///
/// # Atomicity
///
/// On Unix, the rename is atomic, so a concurrent reader sees either
/// the old file or the new one. On Windows, `fs::rename` refuses to
/// overwrite, so this remove-then-renames; this introduces a narrow
/// window (typically microseconds) where the target file doesn't exist.
/// A power failure in that window leaves the game dir without an
/// `override.cfg`, which Godot treats as "no overrides", so the game
/// still boots, just without mods. Acceptable for a config file;
/// doesn't apply to non-recoverable game-state files.
///
/// Parent directory is created if missing so callers don't have to
/// pre-check.
///
/// # Errors
///
/// - [`CrabbyError::Io`] for any leaf I/O failure with the path
///   attached.
/// - [`CrabbyError::Platform`] for rename failures specifically, since
///   platform semantics differ.
pub fn write_atomically(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|source| CrabbyError::io_at(parent.to_path_buf(), source))?;
    }

    let tmp = tmp_path_for(path);

    fs::write(&tmp, text).map_err(|source| CrabbyError::io_at(tmp.clone(), source))?;

    // Remove existing target before rename (Windows `rename` won't
    // overwrite). On failure tidy up the freshly-written .tmp so a
    // retry doesn't fight with a stray file.
    if path.exists()
        && let Err(source) = fs::remove_file(path)
    {
        let _ = fs::remove_file(&tmp);
        return Err(CrabbyError::io_at(path.to_path_buf(), source));
    }

    if let Err(source) = fs::rename(&tmp, path) {
        // Rename failed, so try to clean up the .tmp so the directory
        // isn't left in a half-written state.
        let _ = fs::remove_file(&tmp);
        return Err(CrabbyError::Platform {
            context: format!("renaming {} → {}", tmp.display(), path.display()),
            source: Box::new(source),
        });
    }

    Ok(())
}

/// Compute the `.tmp` sibling path for atomic-write staging.
fn tmp_path_for(target: &Path) -> std::path::PathBuf {
    let mut name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_ascii_only() {
        // Godot's engine-startup config parser has shown odd behavior
        // around non-ASCII content. Every byte we emit must be ASCII.
        let out = render("");
        assert!(out.is_ascii(), "render produced non-ASCII bytes:\n{out}");
    }

    #[test]
    fn render_matches_minimal_shape() {
        // Exact byte shape, since vostok's working override.cfg has
        // this form modulo the autoload name. No leading comments, no
        // trailing blank line after [autoload]. See `render` doc for
        // rationale. Lib is the only [autoload_prepend] entry.
        let out = render("");
        assert_eq!(
            out,
            "[autoload_prepend]\nLib=\"*res://Lib.gd\"\n\n[autoload]\n",
        );
    }

    #[test]
    fn render_includes_lib_autoload() {
        // The in-PCK Lib autoload must be present in every render. If
        // it's missing the modding API never mounts.
        let out = render("");
        assert!(out.contains("Lib=\"*res://Lib.gd\""), "Lib autoload missing:\n{out}");
    }

    #[test]
    fn render_includes_autoload_section_anchor() {
        // Load-bearing: Godot silently ignores [autoload_prepend] if the
        // [autoload] anchor section isn't present.
        for preserved in ["", "[display]\nwidth=1920\n"] {
            let out = render(preserved);
            assert!(
                out.contains("\n[autoload]\n"),
                "missing [autoload] anchor (preserved={})",
                !preserved.is_empty(),
            );
        }
    }

    #[test]
    fn preserved_sections_are_emitted_verbatim() {
        let preserved = "[display]\nwidth=1920\nheight=1080\n";
        let out = render(preserved);
        assert!(out.contains("[display]"));
        assert!(out.contains("width=1920"));
    }

    #[test]
    fn extract_drops_old_autoload_prepend_body() {
        let src = "\
[autoload_prepend]
SomeOld=\"*res://stale.gd\"

[display]
width=1920
";
        let preserved = extract_preserved_sections(src);
        assert!(!preserved.contains("SomeOld"));
        assert!(preserved.contains("[display]"));
        assert!(preserved.contains("width=1920"));
    }

    #[test]
    fn extract_preserves_multiple_non_autoload_sections() {
        let src = "\
[input]
ui_accept=1

[autoload_prepend]
X=\"*x.gd\"

[display]
width=1920
";
        let preserved = extract_preserved_sections(src);
        assert!(preserved.contains("[input]"));
        assert!(preserved.contains("[display]"));
        assert!(!preserved.contains("[autoload_prepend]"));
        assert!(!preserved.contains("X=\"*x.gd\""));
    }

    #[test]
    fn extract_drops_our_placeholder_autoload_section() {
        // The empty [autoload] section is ours; when re-parsing a prior
        // crabby output we must not treat its (absent) body as a user
        // section to preserve.
        let src = "\
[autoload_prepend]
X=\"*x.gd\"

[autoload]

[display]
w=1
";
        let preserved = extract_preserved_sections(src);
        assert!(!preserved.contains("[autoload_prepend]"));
        assert!(!preserved.contains("[autoload]"));
        assert!(preserved.contains("[display]"));
    }

    #[test]
    fn render_roundtrip_is_idempotent() {
        // Re-parsing the output and rendering again must produce the
        // same bytes (no drift across reinstalls).
        let first = render("[display]\nw=1\n");
        let preserved = extract_preserved_sections(&first);
        let second = render(&preserved);
        assert_eq!(
            first, second,
            "render is not idempotent, extract/render round-trip diverges",
        );
    }

    #[test]
    fn extract_strips_leading_bom() {
        // Windows editors can save with a UTF-8 BOM. Feeding it back
        // into `render` would put non-ASCII bytes at the top of
        // override.cfg, which Godot has mishandled before.
        let src = "\u{FEFF}[display]\nwidth=1920\n";
        let preserved = extract_preserved_sections(src);
        assert!(!preserved.contains('\u{FEFF}'), "BOM leaked: {preserved:?}");
        assert!(preserved.contains("[display]"));
    }

    #[test]
    fn extract_ignores_malformed_section_headers() {
        // Bare brackets, empty brackets, or whitespace-only inner text
        // aren't valid headers, so they pass through as verbatim content
        // (the extractor doesn't "enter a section" for them).
        let src = "\
[]
[  ]
[input]
x=1
";
        let preserved = extract_preserved_sections(src);
        // `[input]` IS a valid header, so its body stays.
        assert!(preserved.contains("[input]"));
        assert!(preserved.contains("x=1"));
        // The malformed lines pass through as content (they're not in
        // an owned section).
        assert!(preserved.contains("[]"));
        assert!(preserved.contains("[  ]"));
    }

    #[test]
    fn is_section_header_requires_non_empty_inner() {
        assert!(is_section_header("[input]"));
        assert!(is_section_header("[my section name]"));
        assert!(!is_section_header("[]"));
        assert!(!is_section_header("[ ]"));
        assert!(!is_section_header("["));
        assert!(!is_section_header("]"));
        assert!(!is_section_header("[unclosed"));
        assert!(!is_section_header("not a header [input]"));
    }

    // --- write_atomically tests ----------------------------------------

    use std::path::PathBuf;

    /// Self-cleaning temp dir scoped to the test process + nanoseconds,
    /// so parallel tests don't collide.
    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "crabby-override-cfg-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0),
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn write_atomically_creates_file() {
        let tmp = TempDir::new("create");
        let target = tmp.path.join("override.cfg");
        write_atomically(&target, "hello").expect("write");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        // No stray .tmp left behind.
        assert!(!target.with_file_name("override.cfg.tmp").is_file());
    }

    #[test]
    fn write_atomically_overwrites_existing() {
        let tmp = TempDir::new("overwrite");
        let target = tmp.path.join("override.cfg");
        std::fs::write(&target, "stale").expect("seed");
        write_atomically(&target, "fresh").expect("write");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "fresh");
        assert!(!target.with_file_name("override.cfg.tmp").is_file());
    }

    #[test]
    fn write_atomically_creates_missing_parent() {
        let tmp = TempDir::new("nested");
        let nested = tmp.path.join("deeply").join("nested");
        assert!(!nested.exists());
        let target = nested.join("override.cfg");
        write_atomically(&target, "created").expect("write");
        assert!(target.is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "created");
    }

    #[test]
    fn write_atomically_roundtrips_render_output() {
        // End-to-end sanity: the thing we actually write in production.
        let tmp = TempDir::new("roundtrip");
        let target = tmp.path.join("override.cfg");
        let body = render("[display]\nwidth=1920\n");
        write_atomically(&target, &body).expect("write");
        let read_back = std::fs::read_to_string(&target).unwrap();
        assert_eq!(read_back, body);
        // Re-extracting what we wrote should round-trip cleanly.
        let preserved = extract_preserved_sections(&read_back);
        assert_eq!(render(&preserved), body);
    }
}
