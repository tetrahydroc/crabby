//! Capture git commit + build timestamp at compile time so the
//! launcher and the in-PCK Lib.gd both surface the same identifying
//! info. Lets bug reporters identify which exact build a screenshot
//! came from, and lets mods read `Lib.version()` / `Lib.build_sha()`
//! to do version checks against the running loader.
//!
//! Lives in `crabby-install` (not `crabby-ui`) because `Lib.gd` is
//! assembled here from `LIB_FRAGMENTS`. One source of truth for the
//! identifying triple, both halves of the loader read it.
//!
//! Exposes two env vars to the crate via `cargo:rustc-env`:
//!
//! - `CRABBY_GIT_SHA`  - short commit SHA, with `-dirty` suffix when
//!   the working tree has uncommitted changes. `unknown` when git
//!   isn't available (tarball builds, missing `.git`, missing
//!   `git` binary).
//! - `CRABBY_BUILD_TIME` - ISO 8601 UTC timestamp at compile time.
//!
//! Re-runs on every build (no `cargo:rerun-if-changed` directive)
//! since `CRABBY_BUILD_TIME` is always fresh. The git query is
//! ~1ms so the overhead is negligible.

use std::process::Command;
use std::time::SystemTime;

fn main() {
    let sha = git_sha().unwrap_or_else(|| "unknown".to_string());
    let build_time = build_time_iso8601();
    println!("cargo:rustc-env=CRABBY_GIT_SHA={sha}");
    println!("cargo:rustc-env=CRABBY_BUILD_TIME={build_time}");
}

/// Short git SHA + `-dirty` suffix when the working tree has
/// uncommitted changes. None when git isn't available.
fn git_sha() -> Option<String> {
    let short = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !short.status.success() {
        return None;
    }
    let mut sha = String::from_utf8(short.stdout).ok()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    if working_tree_dirty() {
        sha.push_str("-dirty");
    }
    Some(sha)
}

/// Whether the working tree has uncommitted changes. False when git
/// isn't available (treated as clean - the SHA on its own is then
/// the authoritative identifier).
fn working_tree_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Compile-time UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` form. Uses
/// `SystemTime::now()` so the value is whenever cargo invokes
/// build.rs; subsequent builds bump it.
fn build_time_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_iso8601(secs)
}

/// Minimal Unix-seconds -> ISO 8601 formatter. Avoids the `chrono`
/// dep for one timestamp at build time. Civil-time math via Howard
/// Hinnant's days-from-civil algorithm (public domain).
fn format_unix_iso8601(secs: u64) -> String {
    let days_secs: i64 = secs as i64 / 86_400;
    let time_of_day = secs % 86_400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    let z = days_secs + 719_468; // days since 0000-03-01
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
