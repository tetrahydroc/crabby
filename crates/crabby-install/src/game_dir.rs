//! Detect + validate the Road to Vostok game directory.
//!
//! Detection order (first hit wins):
//! 1. Explicit `--game-dir` argument (handled by caller; this module just validates).
//! 2. `CRABBY_GAME_DIR` env var (also caller-handled).
//! 3. Current working directory.
//! 4. Directory of `env::current_exe()`.
//! 5. Common Steam library locations on the host platform (drive
//!    letters on Windows, conventional paths on Linux/Mac).
//!
//! Validation checks for both `RTV.exe` / `RTV.pck` and the Linux-build name
//! `RTV.x86_64`, so the same code path works on every supported platform.

use std::env;
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};

/// Game-binary filenames that mark an RTV install. Ordered for quick checks.
const GAME_BINARIES: &[&str] = &["RTV.exe", "RTV.x86_64", "RTV"];

/// Game PCK filename. All supported platforms use the same name.
const GAME_PCK: &str = "RTV.pck";

/// Detect the game directory using the fallback chain described in the
/// module docs. Returns the first candidate that validates successfully.
///
/// # Errors
///
/// Returns [`CrabbyError::Platform`] if no candidate validates. The message
/// lists everything tried for diagnosis.
pub fn detect_game_dir() -> Result<PathBuf> {
    let mut tried: Vec<PathBuf> = Vec::new();

    if let Ok(cwd) = env::current_dir() {
        if validate_game_dir(&cwd).is_ok() {
            return Ok(cwd);
        }
        tried.push(cwd);
    }

    if let Ok(exe) = env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        if validate_game_dir(exe_dir).is_ok() {
            return Ok(exe_dir.to_path_buf());
        }
        tried.push(exe_dir.to_path_buf());
    }

    for candidate in steam_library_candidates() {
        if validate_game_dir(&candidate).is_ok() {
            return Ok(candidate);
        }
        tried.push(candidate);
    }

    Err(CrabbyError::Platform {
        context: format!(
            "could not find an RTV install. Tried {} location(s). Pass --game-dir <path> to override.",
            tried.len(),
        ),
        source: "no RTV.exe/RTV.x86_64 + RTV.pck in any candidate dir".into(),
    })
}

/// Enumerate likely Steam library locations on the host. Best-effort,
/// since the list is wide and shallow; every entry is walked, with
/// [`validate_game_dir`] rejecting the ones that don't actually contain
/// the game.
///
/// Windows: every drive letter A-Z probed for the standard library
/// shapes (`SteamLibrary/...` and `Program Files (x86)/Steam/...`).
/// Linux: the well-known `~/.steam` + `~/.local/share/Steam` paths.
/// Mac: `~/Library/Application Support/Steam/...`.
///
/// Public so callers (e.g. the launcher's diagnostic display) can show
/// "tried these" lists without re-running detection.
#[must_use]
pub fn steam_library_candidates() -> Vec<PathBuf> {
    const RTV_REL: &str = "steamapps/common/Road to Vostok";
    let mut out = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for ch in b'A'..=b'Z' {
            let drive = format!("{}:\\", ch as char);
            // Default Steam install location.
            out.push(PathBuf::from(format!("{drive}Program Files (x86)\\Steam")).join(RTV_REL));
            // User-created libraries follow the "SteamLibrary" convention.
            out.push(PathBuf::from(format!("{drive}SteamLibrary")).join(RTV_REL));
            // WSL-style passthrough mounts (so Linux users running the
            // launcher under WSL can see Windows-side installs).
            let wsl_root = format!("/mnt/{}/", (ch as char).to_ascii_lowercase());
            if Path::new(&wsl_root).is_dir() {
                out.push(PathBuf::from(&wsl_root).join("Program Files (x86)/Steam").join(RTV_REL));
                out.push(PathBuf::from(&wsl_root).join("SteamLibrary").join(RTV_REL));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // WSL-style passthrough mounts, for the Linux build of crabby
        // in WSL targeting a Windows-side Steam install.
        for ch in b'a'..=b'z' {
            let mnt = format!("/mnt/{}/", ch as char);
            if Path::new(&mnt).is_dir() {
                out.push(PathBuf::from(&mnt).join("Program Files (x86)/Steam").join(RTV_REL));
                out.push(PathBuf::from(&mnt).join("SteamLibrary").join(RTV_REL));
            }
        }
        if let Ok(home) = env::var("HOME") {
            let home = PathBuf::from(home);
            out.push(home.join(".steam/steam").join(RTV_REL));
            out.push(home.join(".local/share/Steam").join(RTV_REL));
            // Flatpak Steam.
            out.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam").join(RTV_REL));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = env::var("HOME") {
            let home = PathBuf::from(home);
            out.push(home.join("Library/Application Support/Steam").join(RTV_REL));
        }
    }

    out
}

/// Locate the game binary inside a validated RTV install directory.
///
/// Probes [`GAME_BINARIES`] in order and returns the first one that exists.
/// The launcher uses this to spawn the game directly, since the crabby
/// shim is baked into the PCK and the vanilla executable launches the
/// modded game without any wrapper.
///
/// # Errors
///
/// Returns [`CrabbyError::Platform`] when none of the known binary names
/// resolve to a regular file inside `dir`.
pub fn find_game_binary(dir: &Path) -> Result<PathBuf> {
    for name in GAME_BINARIES {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(CrabbyError::Platform {
        context: format!(
            "no game binary in {} (looked for {})",
            dir.display(),
            GAME_BINARIES.join(", "),
        ),
        source: "game binary missing".into(),
    })
}

/// Validate that `dir` looks like an RTV install.
///
/// Requires `RTV.pck` + at least one known game binary. No specific
/// binary is required because users may rename `RTV.exe` (e.g. for
/// exe-swap installs later), but if *nothing* plausible is present
/// validation bails.
///
/// # Errors
///
/// Returns [`CrabbyError::Platform`] when the directory doesn't match.
pub fn validate_game_dir(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(CrabbyError::Platform {
            context: format!("{} is not a directory", dir.display()),
            source: "invalid game dir".into(),
        });
    }

    let pck = dir.join(GAME_PCK);
    if !pck.is_file() {
        return Err(CrabbyError::Platform {
            context: format!("{} has no {GAME_PCK}", dir.display()),
            source: "not an RTV install".into(),
        });
    }

    let has_binary = GAME_BINARIES.iter().any(|name| dir.join(name).is_file());
    if !has_binary {
        return Err(CrabbyError::Platform {
            context: format!(
                "{} has {GAME_PCK} but no known game binary ({})",
                dir.display(),
                GAME_BINARIES.join(", "),
            ),
            source: "game binary missing".into(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "crabby-install-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0),
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn validate_rejects_non_directory() {
        let err = validate_game_dir(Path::new("/does/not/exist")).expect_err("should reject");
        assert!(matches!(err, CrabbyError::Platform { .. }));
    }

    #[test]
    fn validate_rejects_dir_without_pck() {
        let tmp = TempDir::new("no-pck");
        let err = validate_game_dir(&tmp.path).expect_err("should reject");
        assert!(format!("{err}").contains("RTV.pck"));
    }

    #[test]
    fn validate_rejects_pck_without_binary() {
        let tmp = TempDir::new("no-bin");
        std::fs::write(tmp.path.join("RTV.pck"), b"fake").unwrap();
        let err = validate_game_dir(&tmp.path).expect_err("should reject");
        assert!(format!("{err}").contains("game binary"));
    }

    #[test]
    fn validate_accepts_windows_layout() {
        let tmp = TempDir::new("win");
        std::fs::write(tmp.path.join("RTV.pck"), b"fake").unwrap();
        std::fs::write(tmp.path.join("RTV.exe"), b"fake").unwrap();
        validate_game_dir(&tmp.path).expect("should accept");
    }

    #[test]
    fn validate_accepts_linux_layout() {
        let tmp = TempDir::new("linux");
        std::fs::write(tmp.path.join("RTV.pck"), b"fake").unwrap();
        std::fs::write(tmp.path.join("RTV.x86_64"), b"fake").unwrap();
        validate_game_dir(&tmp.path).expect("should accept");
    }
}
