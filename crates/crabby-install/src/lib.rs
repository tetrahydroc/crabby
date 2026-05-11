//! Install / uninstall crabby-loader into a Road to Vostok game directory.
//!
//! All the filesystem placement logic lives here so the CLI, UI, and any
//! future auto-install code path share a single source of truth.
//!
//! # Responsibilities
//!
//! - Detect + validate the game directory
//! - Bake the modded PCK via `crabby-bake` (Lib.gd ships inside the PCK)
//! - Write `override.cfg` preserving any non-`[autoload_prepend]` sections
//! - Remove any orphan on-disk shim left over from older crabby builds
//! - Write an install manifest so uninstall can reverse cleanly and doctor
//!   can diagnose drift
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use crabby_install::{install, InstallOptions};
//!
//! let report = install(&InstallOptions {
//!     game_dir: Path::new("/steam/.../Road to Vostok"),
//!     crabby_version: "0.1.0",
//!     force: false,
//! })?;
//! println!("installed {} file(s)", report.manifest.placed_files.len());
//! # Ok::<(), crabby_error::CrabbyError>(())
//! ```

#![deny(missing_docs)]

mod artifacts;
mod bake_status;
mod doctor;
mod game_dir;
mod install;
mod manifest;
mod override_cfg;
mod pck_backup;
mod uninstall;

/// Crabby workspace version (`Cargo.toml`'s `[workspace.package]
/// version`). The launcher's status bar + diagnostics, the in-PCK
/// `Lib.gd`'s `CRABBY_VERSION`, and `Lib.version()` all read this.
pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short git SHA at compile time (with `-dirty` suffix when the
/// working tree had uncommitted changes). `unknown` when git wasn't
/// available at build time. Surfaced on the Diagnostics tab and on
/// `Lib.build_sha()` so bug reporters and mods can identify the
/// exact build. Source: `crates/crabby-install/build.rs`.
pub const BUILD_GIT_SHA: &str = env!("CRABBY_GIT_SHA");

/// ISO 8601 UTC timestamp captured when crabby-install was compiled.
/// Same surface as `BUILD_GIT_SHA`; pairs with it on Diagnostics +
/// `Lib.build_time()`.
pub const BUILD_TIME: &str = env!("CRABBY_BUILD_TIME");

pub use artifacts::{
    HOOK_PACK_FILE_NAME, LEGACY_SHIM_FILE_NAME, LIB_SOURCE, MANIFEST_DIR, VANILLA_PCK_BACKUP_NAME,
    VANILLA_PCK_NAME,
};
pub use bake_status::{BakeStatus, bake_status, bake_status_from_intents};
pub use doctor::{DoctorReport, InstallStatus, doctor};
pub use game_dir::{
    detect_game_dir, find_game_binary, steam_library_candidates, validate_game_dir,
};
pub use install::{InstallAction, InstallOptions, InstallReport, install};
pub use manifest::InstallManifest;
pub use pck_backup::{
    PckHash, PckState, backup_path, classify_pck, ensure_backup, hash_file, pck_path,
    restore_from_backup,
};
pub use uninstall::{UninstallReport, uninstall};
