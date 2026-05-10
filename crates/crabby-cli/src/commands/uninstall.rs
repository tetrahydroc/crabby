//! `crabby uninstall` - wire argv to [`crabby_install::uninstall`].

use std::path::PathBuf;

use crabby_error::Result;
use crabby_install::{detect_game_dir, uninstall, validate_game_dir};
use tracing::info;

use crate::args::UninstallArgs;

/// Execute the `uninstall` subcommand.
pub fn run(args: &UninstallArgs) -> Result<()> {
    let game_dir = resolve_game_dir(args.game_dir.as_deref())?;
    let report = uninstall(&game_dir)?;

    if !report.had_manifest {
        println!(
            "No crabby install found at {}; nothing to uninstall.",
            game_dir.display(),
        );
        return Ok(());
    }

    info!(
        removed = report.removed_file_count,
        restored = report.restored_override_cfg,
        "uninstall complete",
    );
    println!(
        "Uninstalled. {} file(s) removed.{}",
        report.removed_file_count,
        if report.restored_override_cfg {
            " Pre-install override.cfg restored."
        } else {
            ""
        },
    );
    Ok(())
}

fn resolve_game_dir(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        validate_game_dir(p)?;
        return Ok(p.to_path_buf());
    }
    detect_game_dir()
}
